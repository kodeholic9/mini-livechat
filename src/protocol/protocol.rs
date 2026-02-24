// author: kodeholic (powered by Claude)

use axum::{
    extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State},
    response::Response,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{error, trace, warn};

use crate::config;
use crate::core::{ChannelHub, MediaPeerHub, UserHub};
use crate::error::LiveError;
use crate::protocol::{
    error_code::to_error_code,
    message::{
        AckPayload, ChannelCreatePayload, ChannelDeletePayload, ChannelEventPayload,
        ChannelInfoData, ChannelJoinAckData, ChannelJoinPayload, ChannelLeavePayload,
        ChannelSummary, ChannelUpdatePayload, ErrorPayload, GatewayPacket, HelloPayload,
        IdentifyPayload, MemberInfo, MessageCreatePayload, MessageEventPayload, ReadyPayload,
    },
    opcode::{client, server},
};
use crate::utils::current_timestamp;

// ----------------------------------------------------------------------------
// [공유 상태]
// ----------------------------------------------------------------------------

#[derive(Clone)]
pub struct AppState {
    pub user_hub:       Arc<UserHub>,
    pub channel_hub:    Arc<ChannelHub>,
    pub media_peer_hub: Arc<MediaPeerHub>,
}

// ----------------------------------------------------------------------------
// [WS 진입점]
// ----------------------------------------------------------------------------

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

// ----------------------------------------------------------------------------
// [세션 상태] — 개별 WS 연결마다 보유
// ----------------------------------------------------------------------------

struct Session {
    user_id:         Option<String>,
    current_channel: Option<String>,
    current_ssrc:    Option<u32>,
    current_ufrag:   Option<String>,  // MediaPeerHub 제거용
}

impl Session {
    fn new() -> Self {
        Self { user_id: None, current_channel: None, current_ssrc: None, current_ufrag: None }
    }

    fn is_authenticated(&self) -> bool {
        self.user_id.is_some()
    }
}

// ----------------------------------------------------------------------------
// [핵심] 개별 클라이언트 WS 생명주기
// ----------------------------------------------------------------------------

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (broadcast_tx, mut broadcast_rx) = mpsc::channel::<String>(config::EGRESS_QUEUE_SIZE);

    let mut session = Session::new();

    // HELLO 전송
    let hello = make_packet(server::HELLO, HelloPayload {
        heartbeat_interval: config::HEARTBEAT_INTERVAL_MS,
    });
    if ws_tx.send(Message::Text(hello.into())).await.is_err() {
        return;
    }

    // [rx_loop] broadcast_rx → WS 송신
    let rx_loop = tokio::spawn(async move {
        while let Some(json) = broadcast_rx.recv().await {
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // [tx_loop] WS 수신 → 핸들러 dispatch
    while let Some(msg) = ws_rx.next().await {
        let text = match msg {
            Ok(Message::Text(t))  => t,
            Ok(Message::Close(_)) => break,
            Err(e) => { warn!("WS 에러: {}", e); break; }
            _ => continue,
        };

        let packet: GatewayPacket = match serde_json::from_str(&text) {
            Ok(p)  => p,
            Err(e) => {
                warn!("잘못된 패킷 포맷: {}", e);
                let _ = broadcast_tx.send(error_packet(LiveError::InvalidPayload(e.to_string()))).await;
                continue;
            }
        };

        // IDENTIFY / HEARTBEAT 외에는 인증 필요
        if packet.op != client::IDENTIFY && packet.op != client::HEARTBEAT {
            if !session.is_authenticated() {
                let _ = broadcast_tx.send(error_packet(LiveError::NotAuthenticated)).await;
                continue;
            }
        }

        // 메시지 수신 시 last_seen 갱신
        if let Some(user_id) = &session.user_id {
            if let Some(user) = state.user_hub.get(user_id) {
                user.touch();
            }
        }

        let result = match packet.op {
            client::HEARTBEAT      => handle_heartbeat(&broadcast_tx).await,
            client::IDENTIFY       => handle_identify(&broadcast_tx, &mut session, &state, packet).await,
            client::CHANNEL_CREATE => handle_channel_create(&broadcast_tx, &state, packet).await,
            client::CHANNEL_JOIN   => handle_channel_join(&broadcast_tx, &mut session, &state, packet).await,
            client::CHANNEL_LEAVE  => handle_channel_leave(&broadcast_tx, &mut session, &state, packet).await,
            client::CHANNEL_UPDATE => handle_channel_update(&broadcast_tx, &state, packet).await,
            client::CHANNEL_DELETE => handle_channel_delete(&broadcast_tx, &state, packet).await,
            client::CHANNEL_LIST   => handle_channel_list(&broadcast_tx, &state).await,
            client::CHANNEL_INFO   => handle_channel_info(&broadcast_tx, &state, packet).await,
            client::MESSAGE_CREATE => handle_message_create(&broadcast_tx, &session, &state, packet).await,
            unknown => {
                warn!("알 수 없는 opcode: {}", unknown);
                send(&broadcast_tx, error_packet(LiveError::InvalidOpcode(unknown))).await
            }
        };

        if let Err(e) = result {
            error!("핸들러 에러: {}", e);
        }
    }

    cleanup(&mut session, &state).await;
    rx_loop.abort();
}

// ----------------------------------------------------------------------------
// [op 핸들러들]
// ----------------------------------------------------------------------------

async fn handle_heartbeat(tx: &mpsc::Sender<String>) -> Result<(), LiveError> {
    trace!("HEARTBEAT 수신");
    send(tx, make_no_data(server::HEARTBEAT_ACK)).await
}

async fn handle_identify(
    tx:      &mpsc::Sender<String>,
    session: &mut Session,
    state:   &AppState,
    packet:  GatewayPacket,
) -> Result<(), LiveError> {
    let payload = parse_payload::<IdentifyPayload>(packet.d)?;
    trace!("IDENTIFY - user_id: {}", payload.user_id);

    // Secret Key 검증 (환경변수 LIVECHAT_SECRET 우선, 없으면 DEFAULT_SECRET_KEY)
    let expected = std::env::var("LIVECHAT_SECRET")
        .unwrap_or_else(|_| config::DEFAULT_SECRET_KEY.to_string());
    if payload.token != expected {
        warn!("IDENTIFY 토큰 불일치 - user_id: {}", payload.user_id);
        return send(tx, error_packet(LiveError::InvalidToken)).await;
    }

    state.user_hub.register(&payload.user_id, tx.clone());
    session.user_id = Some(payload.user_id.clone());

    send(tx, make_packet(server::READY, ReadyPayload {
        session_id: format!("sess_{}", current_timestamp()),
        user_id:    payload.user_id,
    })).await
}

async fn handle_channel_create(
    tx:     &mpsc::Sender<String>,
    state:  &AppState,
    packet: GatewayPacket,
) -> Result<(), LiveError> {
    let payload = parse_payload::<ChannelCreatePayload>(packet.d)?;
    trace!("CHANNEL_CREATE - channel_id: {}", payload.channel_id);

    state.channel_hub.create(&payload.channel_id, config::MAX_PEERS_PER_CHANNEL);

    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_CREATE,
        data: serde_json::json!({
            "channel_id":   payload.channel_id,
            "channel_name": payload.channel_name,
        }),
    })).await
}

async fn handle_channel_join(
    tx:      &mpsc::Sender<String>,
    session: &mut Session,
    state:   &AppState,
    packet:  GatewayPacket,
) -> Result<(), LiveError> {
    let payload = parse_payload::<ChannelJoinPayload>(packet.d)?;
    let user_id = session.user_id.as_ref().unwrap().clone();
    trace!("CHANNEL_JOIN - user:{} channel:{}", user_id, payload.channel_id);

    // 1. 채널 정원 체크 + 멤버 등록
    let channel = state.channel_hub.get(&payload.channel_id)
        .ok_or_else(|| LiveError::ChannelNotFound(payload.channel_id.clone()))?;
    channel.add_member(&user_id)?;

    // 2. Endpoint 등록 (ufrag 주키, ssrc는 Track 메타데이터)
    let ep = state.media_peer_hub.insert(&payload.ufrag, &user_id, &payload.channel_id);
    ep.add_track(payload.ssrc, crate::core::TrackKind::Audio);

    session.current_channel = Some(payload.channel_id.clone());
    session.current_ssrc    = Some(payload.ssrc);
    session.current_ufrag   = Some(payload.ufrag.clone());

    // 3. 본인에게 ACK (현재 채널 멤버 목록 포함)
    let active_members = collect_members(&payload.channel_id, state);
    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_JOIN,
        data: serde_json::to_value(ChannelJoinAckData {
            channel_id:     payload.channel_id.clone(),
            active_members,
        }).unwrap_or_default(),
    })).await?;

    // 4. 채널 내 다른 멤버들에게 입장 이벤트 브로드캐스트 (본인 제외)
    let members   = channel.get_members();
    let event_json = make_packet(server::CHANNEL_EVENT, ChannelEventPayload {
        event:      "join".to_string(),
        channel_id: payload.channel_id,
        member:     MemberInfo { user_id: user_id.clone(), ssrc: payload.ssrc },
    });
    state.user_hub.broadcast_to(&members, &event_json, Some(&user_id)).await;

    Ok(())
}

async fn handle_channel_leave(
    tx:      &mpsc::Sender<String>,
    session: &mut Session,
    state:   &AppState,
    packet:  GatewayPacket,
) -> Result<(), LiveError> {
    let payload = parse_payload::<ChannelLeavePayload>(packet.d)?;
    let user_id = session.user_id.as_ref().unwrap().clone();
    trace!("CHANNEL_LEAVE - user:{} channel:{}", user_id, payload.channel_id);

    if session.current_channel.as_deref() != Some(&payload.channel_id) {
        return send(tx, error_packet(LiveError::NotInChannel(payload.channel_id))).await;
    }

    let ssrc  = session.current_ssrc.unwrap();
    let ufrag = session.current_ufrag.clone().unwrap_or_default();

    // 1. 퇴장 이벤트 브로드캐스트 (remove 전에)
    if let Some(channel) = state.channel_hub.get(&payload.channel_id) {
        let members    = channel.get_members();
        let event_json = make_packet(server::CHANNEL_EVENT, ChannelEventPayload {
            event:      "leave".to_string(),
            channel_id: payload.channel_id.clone(),
            member:     MemberInfo { user_id: user_id.clone(), ssrc },
        });
        state.user_hub.broadcast_to(&members, &event_json, Some(&user_id)).await;
        channel.remove_member(&user_id);
    }

    // 2. Endpoint 제거
    state.media_peer_hub.remove(&ufrag);

    session.current_channel = None;
    session.current_ssrc    = None;
    session.current_ufrag   = None;

    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_LEAVE,
        data: serde_json::json!({ "channel_id": payload.channel_id }),
    })).await
}

async fn handle_channel_update(
    tx:     &mpsc::Sender<String>,
    state:  &AppState,
    packet: GatewayPacket,
) -> Result<(), LiveError> {
    let payload = parse_payload::<ChannelUpdatePayload>(packet.d)?;
    trace!("CHANNEL_UPDATE - channel:{}", payload.channel_id);

    let channel = state.channel_hub.get(&payload.channel_id)
        .ok_or_else(|| LiveError::ChannelNotFound(payload.channel_id.clone()))?;

    // TODO: Channel에 name 필드 추가 후 실제 업데이트

    let members    = channel.get_members();
    let event_json = make_packet(server::CHANNEL_EVENT, ChannelEventPayload {
        event:      "update".to_string(),
        channel_id: payload.channel_id.clone(),
        member:     MemberInfo { user_id: "system".to_string(), ssrc: 0 },
    });
    state.user_hub.broadcast_to(&members, &event_json, None).await;

    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_UPDATE,
        data: serde_json::json!({
            "channel_id":   payload.channel_id,
            "channel_name": payload.channel_name,
        }),
    })).await
}

async fn handle_channel_delete(
    tx:     &mpsc::Sender<String>,
    state:  &AppState,
    packet: GatewayPacket,
) -> Result<(), LiveError> {
    let payload = parse_payload::<ChannelDeletePayload>(packet.d)?;
    trace!("CHANNEL_DELETE - channel:{}", payload.channel_id);

    if let Some(channel) = state.channel_hub.get(&payload.channel_id) {
        let members    = channel.get_members();
        let event_json = make_packet(server::CHANNEL_EVENT, ChannelEventPayload {
            event:      "delete".to_string(),
            channel_id: payload.channel_id.clone(),
            member:     MemberInfo { user_id: "system".to_string(), ssrc: 0 },
        });
        state.user_hub.broadcast_to(&members, &event_json, None).await;
    }

    if !state.channel_hub.remove(&payload.channel_id) {
        return send(tx, error_packet(LiveError::ChannelNotFound(payload.channel_id))).await;
    }

    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_DELETE,
        data: serde_json::json!({ "channel_id": payload.channel_id }),
    })).await
}

async fn handle_channel_list(
    tx:    &mpsc::Sender<String>,
    state: &AppState,
) -> Result<(), LiveError> {
    trace!("CHANNEL_LIST 요청");

    let list: Vec<ChannelSummary> = {
        let channels = state.channel_hub.channels.read().unwrap();
        channels.values()
            .map(|ch| ChannelSummary {
                channel_id:   ch.channel_id.clone(),
                member_count: ch.member_count(),
                capacity:     ch.capacity,
                created_at:   ch.created_at,
            })
            .collect()
    };

    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_LIST,
        data: serde_json::to_value(list).unwrap_or_default(),
    })).await
}

async fn handle_channel_info(
    tx:     &mpsc::Sender<String>,
    state:  &AppState,
    packet: GatewayPacket,
) -> Result<(), LiveError> {
    // d: { "channel_id": "CH_001" }
    let channel_id = packet.d
        .as_ref()
        .and_then(|d| d["channel_id"].as_str())
        .ok_or_else(|| LiveError::InvalidPayload("channel_id 필수".to_string()))?;
    trace!("CHANNEL_INFO - channel:{}", channel_id);

    let channel = state.channel_hub.get(channel_id)
        .ok_or_else(|| LiveError::ChannelNotFound(channel_id.to_string()))?;

    let peers = collect_members(channel_id, state);

    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_INFO,
        data: serde_json::to_value(ChannelInfoData {
            channel_id:   channel.channel_id.clone(),
            member_count: channel.member_count(),
            capacity:     channel.capacity,
            created_at:   channel.created_at,
            peers,
        }).unwrap_or_default(),
    })).await
}

async fn handle_message_create(
    tx:      &mpsc::Sender<String>,
    session: &Session,
    state:   &AppState,
    packet:  GatewayPacket,
) -> Result<(), LiveError> {
    let payload = parse_payload::<MessageCreatePayload>(packet.d)?;
    let user_id = session.user_id.as_ref().unwrap().clone();
    trace!("MESSAGE_CREATE - user:{} channel:{}", user_id, payload.channel_id);

    if payload.content.trim().is_empty() {
        return send(tx, error_packet(LiveError::EmptyMessage)).await;
    }
    if payload.content.len() > config::MAX_MESSAGE_LENGTH {
        return send(tx, error_packet(LiveError::MessageTooLong(payload.content.len()))).await;
    }
    if session.current_channel.as_deref() != Some(&payload.channel_id) {
        return send(tx, error_packet(LiveError::MessageNotInChannel(payload.channel_id))).await;
    }

    let channel = state.channel_hub.get(&payload.channel_id)
        .ok_or_else(|| LiveError::ChannelNotFound(payload.channel_id.clone()))?;

    let members    = channel.get_members();
    let event_json = make_packet(server::MESSAGE_EVENT, MessageEventPayload {
        message_id: format!("msg_{}_{}", user_id, current_timestamp()),
        channel_id: payload.channel_id,
        author_id:  user_id,
        content:    payload.content,
        timestamp:  current_timestamp(),
    });

    // 발신자 포함 전원에게 브로드캐스트
    state.user_hub.broadcast_to(&members, &event_json, None).await;

    Ok(())
}

// ----------------------------------------------------------------------------
// [내부 유틸]
// ----------------------------------------------------------------------------

fn parse_payload<T: serde::de::DeserializeOwned>(
    d: Option<serde_json::Value>,
) -> Result<T, LiveError> {
    let value = d.ok_or_else(|| LiveError::InvalidPayload("missing payload".to_string()))?;
    serde_json::from_value(value).map_err(|e| LiveError::InvalidPayload(e.to_string()))
}

fn make_packet(op: u8, payload: impl serde::Serialize) -> String {
    let packet = GatewayPacket::new(op, payload);
    serde_json::to_string(&packet).unwrap_or_default()
}

fn make_no_data(op: u8) -> String {
    serde_json::to_string(&GatewayPacket::no_data(op)).unwrap_or_default()
}

fn error_packet(err: LiveError) -> String {
    make_packet(server::ERROR, ErrorPayload {
        code:   to_error_code(&err),
        reason: err.to_string(),
    })
}

async fn send(tx: &mpsc::Sender<String>, json: String) -> Result<(), LiveError> {
    tx.send(json).await.map_err(|e| LiveError::InternalError(e.to_string()))
}

/// 채널 멤버 목록을 MemberInfo로 변환
/// ssrc는 Endpoint.tracks에서 첫 번째 audio 트랙으로 제공
fn collect_members(channel_id: &str, state: &AppState) -> Vec<MemberInfo> {
    state.media_peer_hub
        .get_channel_endpoints(channel_id)
        .into_iter()
        .map(|ep| {
            let ssrc = ep.tracks.read().unwrap()
                .first()
                .map(|t| t.ssrc)
                .unwrap_or(0);
            MemberInfo { user_id: ep.user_id.clone(), ssrc }
        })
        .collect()
}

/// WS 종료 시 클린업
async fn cleanup(session: &mut Session, state: &AppState) {
    let user_id = match session.user_id.take() {
        Some(uid) => uid,
        None      => return,
    };

    if let (Some(channel_id), Some(ssrc), Some(ufrag)) = (
        session.current_channel.take(),
        session.current_ssrc.take(),
        session.current_ufrag.take(),
    ) {
        trace!("cleanup - user:{} channel:{} ufrag:{}", user_id, channel_id, ufrag);

        if let Some(channel) = state.channel_hub.get(&channel_id) {
            let members    = channel.get_members();
            let event_json = make_packet(server::CHANNEL_EVENT, ChannelEventPayload {
                event:      "leave".to_string(),
                channel_id: channel_id.clone(),
                member:     MemberInfo { user_id: user_id.clone(), ssrc },
            });
            state.user_hub.broadcast_to(&members, &event_json, Some(&user_id)).await;
            channel.remove_member(&user_id);
        }

        state.media_peer_hub.remove(&ufrag);
    }

    state.user_hub.unregister(&user_id);
}
