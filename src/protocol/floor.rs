// author: kodeholic (powered by Claude)
// MBCP TS 24.380 기반 Floor Control 핸들러
//
// 상태머신 (채널별):
//   G: Floor Idle  → Floor Request 수신 → G: Floor Taken
//   G: Floor Taken → Floor Release      → G: Floor Idle  (또는 다음 Queue Grant)
//   G: Floor Taken → Floor Request(高)  → Preempt → G: Floor Taken
//   G: Floor Taken → Floor Ping 무응답  → Revoke  → G: Floor Idle (또는 다음 Queue Grant)
//   G: Floor Taken → 최대 발언 시간 초과 → Revoke  → G: Floor Idle (또는 다음 Queue Grant)
//
// [Send 안전 원칙]
//   std::sync::MutexGuard는 Send가 아니므로 .await 포인트를 넘길 수 없음.
//   모든 lock 사용 패턴: { let mut g = lock(); 상태변경 + 패킷생성; } drop → await
//   decide_next()는 순수 동기 함수로 lock 보유 중 호출, 패킷 Vec 반환
//   dispatch_packets()는 lock 해제 후 호출되는 async 함수

use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{trace, warn};

use crate::config;
use crate::core::{ChannelHub, FloorControl, FloorControlState, FloorIndicator, UserHub};
use crate::error::LiveError;
use crate::trace::{TraceDir, TraceEvent, TraceHub};
use crate::protocol::message::{
    FloorGrantedPayload, FloorIdlePayload, FloorIndicatorDto,
    FloorPingPayload, FloorPongPayload,
    FloorQueuePosInfoPayload, FloorReleasePayload, FloorRequestPayload, FloorRevokePayload,
    FloorTakenPayload, GatewayPacket,
};
#[allow(unused_imports)]
use crate::protocol::opcode::{client, server};

// ----------------------------------------------------------------------------
// [DTO 변환]
// ----------------------------------------------------------------------------

fn indicator_to_dto(ind: &FloorIndicator) -> FloorIndicatorDto {
    match ind {
        FloorIndicator::Normal        => FloorIndicatorDto::Normal,
        FloorIndicator::Broadcast     => FloorIndicatorDto::Broadcast,
        FloorIndicator::ImminentPeril => FloorIndicatorDto::ImminentPeril,
        FloorIndicator::Emergency     => FloorIndicatorDto::Emergency,
    }
}

fn dto_to_indicator(dto: &FloorIndicatorDto) -> FloorIndicator {
    match dto {
        FloorIndicatorDto::Normal        => FloorIndicator::Normal,
        FloorIndicatorDto::Broadcast     => FloorIndicator::Broadcast,
        FloorIndicatorDto::ImminentPeril => FloorIndicator::ImminentPeril,
        FloorIndicatorDto::Emergency     => FloorIndicator::Emergency,
    }
}

// ----------------------------------------------------------------------------
// [내부 유틸]
// ----------------------------------------------------------------------------

fn make_packet(op: u8, payload: impl serde::Serialize) -> String {
    let packet = GatewayPacket::new(op, payload);
    serde_json::to_string(&packet).unwrap_or_default()
}

async fn send(tx: &mpsc::Sender<String>, json: String) -> Result<(), LiveError> {
    tx.send(json).await.map_err(|e| LiveError::InternalError(e.to_string()))
}

/// Revoke 후 상태 전이 결정 (순수 동기) — MutexGuard 보유 중에 호출
///
/// 패턴:
///   let packets = { let mut g = lock(); decide_next(...); };  // lock 해제
///   dispatch_packets(packets, ...).await;                     // await
fn decide_next(
    channel_id: &str,
    floor:      &mut FloorControl,
    members:    &std::collections::HashSet<String>,
) -> Vec<(Option<String>, Option<String>, String)> {
    let _ = members;
    let mut packets: Vec<(Option<String>, Option<String>, String)> = Vec::new();

    if let Some(next) = floor.dequeue_next() {
        floor.grant(next.user_id.clone(), next.priority, next.indicator.clone());

        // 다음 holder에게만 FLOOR_GRANTED
        packets.push((Some(next.user_id.clone()), None, make_packet(server::FLOOR_GRANTED, FloorGrantedPayload {
            channel_id: channel_id.to_string(),
            user_id:    next.user_id.clone(),
            duration:   config::FLOOR_MAX_TAKEN_MS,
        })));

        // FLOOR_TAKEN: 나머지 멤버에게만 (holder 제외)
        packets.push((None, Some(next.user_id.clone()), make_packet(server::FLOOR_TAKEN, FloorTakenPayload {
            channel_id: channel_id.to_string(),
            user_id:    next.user_id.clone(),
            indicator:  indicator_to_dto(&next.indicator),
        })));

        trace!("Floor Queue → Grant: channel={} user={}", channel_id, next.user_id);
    } else {
        floor.clear_taken();

        // FLOOR_IDLE: 전체에게
        packets.push((None, None, make_packet(server::FLOOR_IDLE, FloorIdlePayload {
            channel_id: channel_id.to_string(),
        })));

        trace!("Floor Idle: channel={}", channel_id);
    }

    packets
}

/// decide_next 결과 전송 (lock 해제 후 호출)
/// (target, exclude, json)
///   target=None  : 전체 브로드캐스트
///   target=Some  : 특정 유저에게만
///   exclude=Some : 브로드캐스트 시 제외할 유저
async fn dispatch_packets(
    packets:  Vec<(Option<String>, Option<String>, String)>,
    members:  &std::collections::HashSet<String>,
    user_hub: &Arc<UserHub>,
) {
    for (target, exclude, json) in packets {
        match target {
            Some(uid) => {
                if let Some(user) = user_hub.get(&uid) {
                    let _ = user.tx.send(json).await;
                }
            }
            None => {
                user_hub.broadcast_to(members, &json, exclude.as_deref()).await;
            }
        }
    }
}

// ----------------------------------------------------------------------------
// [op 핸들러들]
// ----------------------------------------------------------------------------

/// op: FLOOR_REQUEST (30) — PTT 누름, 발언권 요청
pub async fn handle_floor_request(
    tx:          &mpsc::Sender<String>,
    user_id:     &str,
    user_hub:    &Arc<UserHub>,
    channel_hub: &Arc<ChannelHub>,
    trace_hub:   &Arc<TraceHub>,
    packet:      GatewayPacket,
) -> Result<(), LiveError> {
    let payload    = parse_payload::<FloorRequestPayload>(packet.d)?;
    let channel_id = payload.channel_id.clone();

    trace!("FLOOR_REQUEST user={} channel={}", user_id, channel_id);

    let priority = payload.priority.unwrap_or_else(|| {
        user_hub.get(user_id)
            .map(|u| u.priority)
            .unwrap_or(config::FLOOR_PRIORITY_DEFAULT)
    });
    let indicator = payload.indicator.as_ref()
        .map(dto_to_indicator)
        .unwrap_or(FloorIndicator::Normal);

    let channel = channel_hub.get(&channel_id)
        .ok_or_else(|| LiveError::ChannelNotFound(channel_id.clone()))?;
    let members = channel.get_members();

    // 패턴: lock → 상태 변경 + 패킷 생성 → drop → await
    enum Action {
        Granted  { granted_json: String, taken_json: String },
        Preempt  { revoke_json: String, granted_json: String, taken_json: String, old_holder: String },
        Queued   { pos_json: String },
    }

    let action = {
        let mut floor = channel.floor.lock().unwrap();
        match floor.state {
            FloorControlState::Idle => {
                floor.grant(user_id.to_string(), priority, indicator.clone());
                Action::Granted {
                    granted_json: make_packet(server::FLOOR_GRANTED, FloorGrantedPayload {
                        channel_id: channel_id.clone(),
                        user_id:    user_id.to_string(),
                        duration:   config::FLOOR_MAX_TAKEN_MS,
                    }),
                    taken_json: make_packet(server::FLOOR_TAKEN, FloorTakenPayload {
                        channel_id: channel_id.clone(),
                        user_id:    user_id.to_string(),
                        indicator:  indicator_to_dto(&indicator),
                    }),
                }
            }
            FloorControlState::Taken => {
                if floor.can_preempt(priority, &indicator) {
                    let old_holder = floor.floor_taken_by.clone().unwrap_or_default();
                    let revoke_json = make_packet(server::FLOOR_REVOKE, FloorRevokePayload {
                        channel_id: channel_id.clone(),
                        cause:      "preempted".to_string(),
                    });
                    floor.grant(user_id.to_string(), priority, indicator.clone());
                    Action::Preempt {
                        revoke_json,
                        granted_json: make_packet(server::FLOOR_GRANTED, FloorGrantedPayload {
                            channel_id: channel_id.clone(),
                            user_id:    user_id.to_string(),
                            duration:   config::FLOOR_MAX_TAKEN_MS,
                        }),
                        taken_json: make_packet(server::FLOOR_TAKEN, FloorTakenPayload {
                            channel_id: channel_id.clone(),
                            user_id:    user_id.to_string(),
                            indicator:  indicator_to_dto(&indicator),
                        }),
                        old_holder,
                    }
                } else {
                    floor.enqueue(user_id.to_string(), priority, indicator);
                    let pos  = floor.queue_position(user_id).unwrap_or(1);
                    let size = floor.queue.len();
                    Action::Queued {
                        pos_json: make_packet(server::FLOOR_QUEUE_POS_INFO, FloorQueuePosInfoPayload {
                            channel_id:     channel_id.clone(),
                            queue_position: pos,
                            queue_size:     size,
                        }),
                    }
                }
            }
        }
        // MutexGuard drop here
    };

    // lock 해제 후 await
    match action {
        Action::Granted { granted_json, taken_json } => {
            send(tx, granted_json).await?;
            user_hub.broadcast_to(&members, &taken_json, Some(user_id)).await;
            trace!("Floor Granted (Idle→Taken): channel={} user={}", channel_id, user_id);
            trace_hub.publish(TraceEvent::new(
                TraceDir::Out, Some(&channel_id), Some(user_id),
                server::FLOOR_GRANTED, "FLOOR_GRANTED",
                format!("user={} priority={}", user_id, priority),
            ));
        }
        Action::Preempt { revoke_json, granted_json, taken_json, old_holder } => {
            if let Some(holder_user) = user_hub.get(&old_holder) {
                let _ = holder_user.tx.send(revoke_json).await;
            }
            send(tx, granted_json).await?;
            user_hub.broadcast_to(&members, &taken_json, Some(user_id)).await;
            warn!("Floor Preempted: channel={} old={} new={}", channel_id, old_holder, user_id);
            trace_hub.publish(TraceEvent::new(
                TraceDir::Out, Some(&channel_id), Some(user_id),
                server::FLOOR_GRANTED, "FLOOR_GRANTED(PREEMPT)",
                format!("new={} old={} priority={}", user_id, old_holder, priority),
            ));
        }
        Action::Queued { pos_json } => {
            send(tx, pos_json).await?;
            trace_hub.publish(TraceEvent::new(
                TraceDir::Out, Some(&channel_id), Some(user_id),
                server::FLOOR_QUEUE_POS_INFO, "FLOOR_QUEUED",
                format!("user={} priority={}", user_id, priority),
            ));
        }
    }

    Ok(())
}

/// op: FLOOR_RELEASE (31) — PTT 놓음, 발언권 반납
pub async fn handle_floor_release(
    _tx:         &mpsc::Sender<String>,
    user_id:     &str,
    user_hub:    &Arc<UserHub>,
    channel_hub: &Arc<ChannelHub>,
    trace_hub:   &Arc<TraceHub>,
    packet:      GatewayPacket,
) -> Result<(), LiveError> {
    let payload    = parse_payload::<FloorReleasePayload>(packet.d)?;
    let channel_id = &payload.channel_id;

    trace!("FLOOR_RELEASE user={} channel={}", user_id, channel_id);

    let channel = channel_hub.get(channel_id)
        .ok_or_else(|| LiveError::ChannelNotFound(channel_id.clone()))?;
    let members = channel.get_members();

    let packets = {
        let mut floor = channel.floor.lock().unwrap();
        if floor.floor_taken_by.as_deref() != Some(user_id) {
            warn!("FLOOR_RELEASE non-holder: user={} channel={}", user_id, channel_id);
            return Ok(());
        }
        decide_next(channel_id, &mut floor, &members)
        // MutexGuard drop here
    };

    trace_hub.publish(TraceEvent::new(
        TraceDir::Out, Some(channel_id), Some(user_id),
        server::FLOOR_IDLE, "FLOOR_RELEASE→IDLE",
        format!("user={}", user_id),
    ));

    dispatch_packets(packets, &members, user_hub).await;
    Ok(())
}

/// op: FLOOR_PING (32) — C→S, holder 생존 신호
/// last_ping_at 갱신 후 FLOOR_PONG(116) 응답
pub async fn handle_floor_ping(
    tx:          &mpsc::Sender<String>,
    user_id:     &str,
    channel_hub: &Arc<ChannelHub>,
    packet:      GatewayPacket,
) -> Result<(), LiveError> {
    let payload    = parse_payload::<FloorPingPayload>(packet.d)?;
    let channel_id = &payload.channel_id;

    let channel = channel_hub.get(channel_id)
        .ok_or_else(|| LiveError::ChannelNotFound(channel_id.clone()))?;

    {
        let mut floor = channel.floor.lock().unwrap();
        if floor.floor_taken_by.as_deref() != Some(user_id) {
            return Ok(());
        }
        floor.on_ping();
        trace!("Floor Ping rcv: channel={} user={}", channel_id, user_id);
        // MutexGuard drop
    }

    // FLOOR_PONG 응답
    send(tx, make_packet(server::FLOOR_PONG, FloorPongPayload {
        channel_id: channel_id.clone(),
    })).await
}

// ----------------------------------------------------------------------------
// [Floor 타임아웃 체크] — zombie reaper에서 주기적으로 호출
// run_floor_ping_task 대체 — 별도 태스크 없이 reaper와 통합
// ----------------------------------------------------------------------------

/// 모든 채널의 Floor 상태를 순회하며 타임아웃/max_duration Revoke 처리
/// trace_hub: Some이면 Revoke 이벤트 publish, None이면 생략
pub async fn check_floor_timeouts(
    user_hub:    &Arc<UserHub>,
    channel_hub: &Arc<ChannelHub>,
    trace_hub:   Option<&Arc<TraceHub>>,
) {
    let channel_ids: Vec<String> = {
        channel_hub.channels.read().unwrap().keys().cloned().collect()
    };

    for channel_id in channel_ids {
        let channel = match channel_hub.get(&channel_id) {
            Some(ch) => ch,
            None     => continue,
        };

        enum Action {
            Skip,
            Revoke {
                cause:       String,
                holder:      String,
                revoke_json: String,
                packets:     Vec<(Option<String>, Option<String>, String)>,
                members:     std::collections::HashSet<String>,
            },
        }

        let action = {
            let mut floor = channel.floor.lock().unwrap();

            if floor.state != FloorControlState::Taken {
                Action::Skip
            } else if floor.is_max_taken_exceeded() {
                let holder      = floor.floor_taken_by.clone().unwrap_or_default();
                let revoke_json = make_packet(server::FLOOR_REVOKE, FloorRevokePayload {
                    channel_id: channel_id.clone(),
                    cause:      "max_duration".to_string(),
                });
                let members = channel.get_members();
                let packets = decide_next(&channel_id, &mut floor, &members);
                Action::Revoke { cause: "max_duration".to_string(), holder, revoke_json, packets, members }
            } else if floor.is_ping_timeout() {
                let holder      = floor.floor_taken_by.clone().unwrap_or_default();
                let revoke_json = make_packet(server::FLOOR_REVOKE, FloorRevokePayload {
                    channel_id: channel_id.clone(),
                    cause:      "ping_timeout".to_string(),
                });
                let members = channel.get_members();
                let packets = decide_next(&channel_id, &mut floor, &members);
                Action::Revoke { cause: "ping_timeout".to_string(), holder, revoke_json, packets, members }
            } else {
                Action::Skip
            }
            // MutexGuard drop
        };

        if let Action::Revoke { cause, holder, revoke_json, packets, members } = action {
            warn!("Floor Revoke ({}): channel={} user={}", cause, channel_id, holder);
            if let Some(user) = user_hub.get(&holder) {
                let _ = user.tx.send(revoke_json).await;
            }
            if let Some(th) = trace_hub {
                th.publish(TraceEvent::new(
                    TraceDir::Sys, Some(&channel_id), Some(&holder),
                    server::FLOOR_REVOKE, "FLOOR_REVOKE",
                    format!("cause={} user={}", cause, holder),
                ));
            }
            dispatch_packets(packets, &members, user_hub).await;
        }
    }
}

// ----------------------------------------------------------------------------
// [WS cleanup 연동]
// ----------------------------------------------------------------------------

/// WS 연결 종료 시 해당 user의 Floor 상태 정리
pub async fn on_user_disconnect(
    user_id:     &str,
    channel_id:  &str,
    user_hub:    &Arc<UserHub>,
    channel_hub: &Arc<ChannelHub>,
) {
    let channel = match channel_hub.get(channel_id) {
        Some(ch) => ch,
        None     => return,
    };
    let members = channel.get_members();

    let packets = {
        let mut floor = channel.floor.lock().unwrap();
        floor.remove_from_queue(user_id);
        if floor.floor_taken_by.as_deref() == Some(user_id) {
            warn!("Floor Disconnect Revoke: channel={} user={}", channel_id, user_id);
            decide_next(channel_id, &mut floor, &members)
        } else {
            vec![]
        }
        // MutexGuard drop here
    };

    dispatch_packets(packets, &members, user_hub).await;
}

// ----------------------------------------------------------------------------
// [내부 파싱 유틸]
// ----------------------------------------------------------------------------

fn parse_payload<T: serde::de::DeserializeOwned>(
    d: Option<serde_json::Value>,
) -> Result<T, LiveError> {
    let value = d.ok_or_else(|| LiveError::InvalidPayload("missing payload".to_string()))?;
    serde_json::from_value(value).map_err(|e| LiveError::InvalidPayload(e.to_string()))
}
