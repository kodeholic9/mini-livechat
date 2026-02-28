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
use crate::trace::{TraceDir, TraceEvent, TraceHub};
use crate::protocol::{
    floor,
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
    pub server_cert:    Arc<crate::media::ServerCert>,
    pub trace_hub:      Arc<TraceHub>,
    pub udp_port:       u16,  // SDP answer candidate 포트
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

        // trace: C→S 수신 이벤트 publish
        let trace_channel = session.current_channel.as_deref();
        let trace_user    = session.user_id.as_deref();
        publish_in_event(&state.trace_hub, packet.op, trace_channel, trace_user);

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
            client::FLOOR_REQUEST  => floor::handle_floor_request(&broadcast_tx, session.user_id.as_deref().unwrap(), &state.user_hub, &state.channel_hub, &state.trace_hub, packet).await,
            client::FLOOR_RELEASE  => floor::handle_floor_release(&broadcast_tx, session.user_id.as_deref().unwrap(), &state.user_hub, &state.channel_hub, &state.trace_hub, packet).await,
            client::FLOOR_PING     => floor::handle_floor_ping(&broadcast_tx, session.user_id.as_deref().unwrap(), &state.channel_hub, packet).await,
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

    let priority = payload.priority.unwrap_or(config::FLOOR_PRIORITY_DEFAULT);
    state.user_hub.register(&payload.user_id, tx.clone(), priority);
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

    state.channel_hub.create(
        &payload.channel_id,
        &payload.freq,
        &payload.channel_name,
        config::MAX_PEERS_PER_CHANNEL,
    );

    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_CREATE,
        data: serde_json::json!({
            "channel_id":   payload.channel_id,
            "freq":         payload.freq,
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

    // 3. SDP answer 생성 (offer가 있을 때만)
    // server_ufrag: 서버가 생성한 ICE ufrag → MediaPeerHub 등록 키
    // STUN USERNAME = "server_ufrag:client_ufrag" 구조이므로 서버 ufrag로 조회해야 함
    let (sdp_answer, ep_ufrag, ep_pwd) = match payload.sdp_offer.as_deref() {
        Some(offer) => {
            // offer의 a=setup 값 확인용 로그 (DTLS 역할 디버그)
            let offer_setup = offer.lines()
                .find(|l| l.starts_with("a=setup:"))
                .unwrap_or("a=setup:(없음)");
            trace!("[sdp] offer a=setup: {}", offer_setup);
            let (sdp, server_ufrag, server_pwd) = build_sdp_answer(offer, &state.server_cert.fingerprint, state.udp_port);
            trace!("[sdp] answer built ufrag={} (offer_setup={})", server_ufrag, offer_setup);
            (Some(sdp), server_ufrag, server_pwd)
        }
        None => (None, payload.ufrag.clone(), String::new()),
    };

    // 2. Endpoint 등록 (server_ufrag 주키, ice_pwd 포함)
    let ep = state.media_peer_hub.insert(&ep_ufrag, &ep_pwd, &user_id, &payload.channel_id);
    ep.add_track(payload.ssrc, crate::core::TrackKind::Audio);

    session.current_channel = Some(payload.channel_id.clone());
    session.current_ssrc    = Some(payload.ssrc);
    session.current_ufrag   = Some(ep_ufrag);

    // 4. 본인에게 ACK (SDP answer + 현재 채널 멤버 목록 포함)
    let active_members = collect_members(&payload.channel_id, state);
    send(tx, make_packet(server::ACK, AckPayload {
        op:   client::CHANNEL_JOIN,
        data: serde_json::to_value(ChannelJoinAckData {
            channel_id:     payload.channel_id.clone(),
            sdp_answer,
            active_members,
        }).unwrap_or_default(),
    })).await?;

    // 5. 채널 내 다른 멤버들에게 입장 이벤트 브로드캐스트 (본인 제외)
    let members   = channel.get_members();
    let event_json = make_packet(server::CHANNEL_EVENT, ChannelEventPayload {
        event:      "join".to_string(),
        channel_id: payload.channel_id.clone(),
        member:     MemberInfo { user_id: user_id.clone(), ssrc: payload.ssrc },
    });
    state.user_hub.broadcast_to(&members, &event_json, Some(&user_id)).await;

    // 6. Floor Taken 상태라면 신규 입장자에게 FLOOR_TAKEN 전송
    //    MutexGuard가 await를 걸치면 Send 불만족 → 동기 블록에서 패킷 문자열만 추출,
    //    Guard는 블록 끝에서 drop되고 await는 그 다음에 실행됨
    let floor_taken_packet: Option<String> = {
        use crate::core::FloorControlState;
        use crate::protocol::message::{FloorTakenPayload, FloorIndicatorDto};
        let floor = channel.floor.lock().unwrap();
        if floor.state == FloorControlState::Taken {
            floor.floor_taken_by.as_ref().map(|holder| {
                make_packet(server::FLOOR_TAKEN, FloorTakenPayload {
                    channel_id: payload.channel_id.clone(),
                    user_id:    holder.clone(),
                    indicator:  FloorIndicatorDto::Normal,
                })
            })
        } else {
            None
        }
    }; // ← MutexGuard 여기서 drop
    if let Some(pkt) = floor_taken_packet {
        let _ = tx.send(pkt).await;
    }

    state.trace_hub.publish(TraceEvent::new(
        TraceDir::Sys,
        Some(&payload.channel_id),
        Some(&user_id),
        server::CHANNEL_EVENT,
        "CHANNEL_JOIN",
        format!("user={} ssrc={}", user_id, payload.ssrc),
    ));

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
        let mut list: Vec<ChannelSummary> = channels.values()
            .map(|ch| ChannelSummary {
                channel_id:   ch.channel_id.clone(),
                freq:         ch.freq.clone(),
                name:         ch.name.clone(),
                member_count: ch.member_count(),
                capacity:     ch.capacity,
                created_at:   ch.created_at,
            })
            .collect();
        // freq 오름쉠으로 정렬 (0001, 0112, ...)
        list.sort_by(|a, b| a.freq.cmp(&b.freq));
        list
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
            freq:         channel.freq.clone(),
            name:         channel.name.clone(),
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
        code:   err.code(),
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

// ----------------------------------------------------------------------------
// [SDP Answer 생성]
//
// 브라우저 offer를 파싱해서 필요한 라인만 추출 후 서버 answer를 조립합니다.
// webrtc-sdp 크레이트 대신 직접 조립 — 버전 호환성 문제 방지.
//
// answer 구조:
//   - offer의 미디어 라인(m=, a=rtpmap 등) 미러링
//   - 서버 ICE ufrag/pwd (랜덤 4/22자)
//   - 서버 DTLS fingerprint (ServerCert에서)
//   - a=setup:passive (서버는 항상 passive)
// ----------------------------------------------------------------------------

/// SDP answer 조립 후 (sdp_string, server_ufrag, server_pwd) 반환
/// server_ufrag: MediaPeerHub 등록 키
/// server_pwd:   STUN MESSAGE-INTEGRITY 서명 키
fn build_sdp_answer(offer: &str, fingerprint: &str, udp_port_arg: u16) -> (String, String, String) {
    // --------------------------------------------------------------------
    // SDP Answer 조립 규칙
    //
    // 1. 세션 헤더: v=, o=, s=, t=, BUNDLE 그룹, ice-lite
    // 2. 미디어 섹션: offer의 audio 섹션 1개만 처리
    //    - m= 포트를 SERVER_UDP_PORT 로 교체 (offer의 9는 더미)
    //    - 코덱 라인(a=rtpmap, a=fmtp, a=extmap, a=mid 등) 미러링
    //    - ICE/DTLS/방향 라인은 서버 값으로 교체
    //    - sendrecv/sendonly → recvonly
    // 3. ICE candidate: 라우팅 테이블 기반 로컬 IP 자동 감지
    // --------------------------------------------------------------------

    let session_id   = crate::utils::current_timestamp();
    let server_ufrag = random_ice_string(16);
    let server_pwd   = random_ice_string(22);
    let local_ip     = crate::protocol::get_advertise_ip(); // CLI 설정 or 자동 감지
    let udp_port     = udp_port_arg;

    // offer에서 audio 미디어 섹션의 코덱 관련 라인만 수집
    // (ICE/DTLS/방향/c= 제외 — 서버 값으로 교체)
    let skip_prefixes = [
        "a=ice-", "a=fingerprint", "a=setup", "a=candidate",
        "a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive",
        "a=rtcp-mux", "a=rtcp-rsize", "c=",
    ];

    let mut mid = "0".to_string();
    let mut m_line = format!("m=audio {} UDP/TLS/RTP/SAVPF 111", udp_port); // 폴백
    let mut codec_lines: Vec<String> = Vec::new();
    let mut in_audio = false;

    for line in offer.lines() {
        if line.starts_with("m=audio") {
            in_audio = true;
            // m= 라인: 포트만 서버 UDP 포트로 교체, 나머지(코덱 목록)는 그대로
            // "m=audio 9 UDP/TLS/RTP/SAVPF 111 63 ..." → "m=audio 10000 UDP/TLS/RTP/SAVPF 111 63 ..."
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            if parts.len() == 4 {
                m_line = format!("m=audio {} {} {}", udp_port, parts[2], parts[3]);
            }
            continue;
        }
        if line.starts_with("m=") {
            in_audio = false;
            continue;
        }
        if !in_audio { continue; }

        if skip_prefixes.iter().any(|p| line.starts_with(p)) { continue; }

        // a=mid 값 캡처 (BUNDLE에서 필요)
        if line.starts_with("a=mid:") {
            mid = line["a=mid:".len()..].trim().to_string();
        }

        codec_lines.push(line.to_string());
    }

    // answer 조립
    let mut sdp = String::new();

    // --- 세션 헤더 ---
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=mini-livechat {0} {0} IN IP4 {1}\r\n", session_id, local_ip));
    sdp.push_str("s=-\r\n");
    sdp.push_str("t=0 0\r\n");
    sdp.push_str(&format!("a=group:BUNDLE {}\r\n", mid)); // BUNDLE 필수
    sdp.push_str("a=ice-lite\r\n");                       // 서버는 ICE Lite

    // --- 미디어 섹션 (audio 1개) ---
    // m= 라인: offer에서 코덱 목록 그대로, 포트만 서버 UDP 포트로 교체
    sdp.push_str(&m_line);
    sdp.push_str("\r\n");
    sdp.push_str(&format!("c=IN IP4 {}\r\n", local_ip));

    // ICE 크리덴셜
    sdp.push_str(&format!("a=ice-ufrag:{}\r\n", server_ufrag));
    sdp.push_str(&format!("a=ice-pwd:{}\r\n", server_pwd));

    // DTLS
    sdp.push_str(&format!("a=fingerprint:{}\r\n", fingerprint));
    sdp.push_str("a=setup:passive\r\n"); // 서버는 항상 passive

    // 미디어 속성
    sdp.push_str("a=rtcp-mux\r\n");
    sdp.push_str("a=rtcp-rsize\r\n");
    // sendrecv 사용: recvonly로 하면 일부 브라우저/버전에서 DTLS를 시작하지 않는 문제 발생
    // 실제 미디어 방향은 애플리케이션 레이어(Floor Control)에서 제어
    sdp.push_str("a=sendrecv\r\n");

    // offer에서 미러링한 코덱 라인들
    for line in &codec_lines {
        sdp.push_str(line);
        sdp.push_str("\r\n");
    }

    // ICE candidate (ICE Lite이므로 host 후보 1개만)
    sdp.push_str(&format!(
        "a=candidate:1 1 udp 2113937151 {} {} typ host generation 0\r\n",
        local_ip, udp_port
    ));
    sdp.push_str("a=end-of-candidates\r\n");

    (sdp, server_ufrag, server_pwd)
}

/// 라우팅 테이블 기반 로컬 IP 자동 감지
/// UDP 소켓으로 8.8.8.8:80 connect (실제 패킷 없음) → local_addr() 조회
/// 멀티홈 환경에서도 외부 통신에 실제로 쓰이는 인터페이스 IP가 정확히 반환됨
pub fn detect_local_ip() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| { s.connect("8.8.8.8:80")?; s.local_addr() })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| {
            tracing::warn!("로컬 IP 감지 실패 — 127.0.0.1 폴백");
            "127.0.0.1".to_string()
        })
}

/// ICE ufrag/pwd용 랜덤 문자열 생성 (alphanumeric)
/// - rand 크레이트 기반 CSPRNG 사용 (xorshift 대비 충돌 안전)
/// - ufrag: 16자 권장 (RFC 8445 범위 4~256, 62^16 ≈ 4.7×10^28)
/// - pwd:   22자 (RFC 최솟값 준수)
fn random_ice_string(len: usize) -> String {
    use rand::Rng;
    let charset: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| charset[rng.gen_range(0..charset.len())] as char)
        .collect()
}

// ----------------------------------------------------------------------------
// [Trace 유틸]
// ----------------------------------------------------------------------------

/// C→S 수신 패킷을 TraceHub에 publish (HEARTBEAT 제외)
fn publish_in_event(
    trace_hub:  &TraceHub,
    op:         u8,
    channel_id: Option<&str>,
    user_id:    Option<&str>,
) {
    // HEARTBEAT는 노이즈 — 제외
    if op == client::HEARTBEAT { return; }

    let (op_name, summary) = op_meta_in(op, user_id);
    trace_hub.publish(TraceEvent::new(
        TraceDir::In,
        channel_id,
        user_id,
        op,
        op_name,
        summary,
    ));
}

/// C→S opcode → (이름, 요약)
fn op_meta_in(op: u8, user_id: Option<&str>) -> (&'static str, String) {
    let uid = user_id.unwrap_or("-");
    match op {
        client::IDENTIFY       => ("IDENTIFY",       format!("user={}", uid)),
        client::CHANNEL_CREATE => ("CHANNEL_CREATE", format!("user={}", uid)),
        client::CHANNEL_JOIN   => ("CHANNEL_JOIN",   format!("user={}", uid)),
        client::CHANNEL_LEAVE  => ("CHANNEL_LEAVE",  format!("user={}", uid)),
        client::CHANNEL_LIST   => ("CHANNEL_LIST",   format!("user={}", uid)),
        client::MESSAGE_CREATE => ("MESSAGE_CREATE", format!("user={}", uid)),
        client::FLOOR_REQUEST  => ("FLOOR_REQUEST",  format!("user={}", uid)),
        client::FLOOR_RELEASE  => ("FLOOR_RELEASE",  format!("user={}", uid)),
        client::FLOOR_PING     => ("FLOOR_PING",     format!("user={}", uid)),
        _                      => ("UNKNOWN",         format!("op={} user={}", op, uid)),
    }
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

        // Floor Control 정리 (holder면 Revoke, 대기열이면 제거)
        floor::on_user_disconnect(&user_id, &channel_id, &state.user_hub, &state.channel_hub).await;
    }

    state.user_hub.unregister(&user_id);
}
