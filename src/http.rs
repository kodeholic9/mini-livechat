// author: kodeholic (powered by Claude)
//
// HTTP REST API 핸들러
//
// 일반 조회
//   GET /channels          → 채널 목록 (id, member_count, capacity)
//   GET /channels/{id}     → 채널 상세 + peer 목록 (user_id, ssrc)
//
// Admin 조회
//   GET /admin/status                  → 서버 상태 요약 (uptime, 연결 수)
//   GET /admin/users                   → User 전체 목록
//   GET /admin/users/{user_id}         → User 상세
//   GET /admin/channels                → Channel 전체 목록 (Floor 상태 포함)
//   GET /admin/channels/{channel_id}   → Channel 상세
//   GET /admin/peers                   → Endpoint 전체 목록
//   GET /admin/peers/{ufrag}           → Endpoint 상세
//
// Admin 조작
//   POST /admin/floor-revoke/{channel_id} → Floor 강제 revoke

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{ChannelHub, FloorControlState, MediaPeerHub, UserHub};
use crate::utils::current_timestamp;

// ----------------------------------------------------------------------------
// [공유 상태] HTTP 핸들러용 — WS AppState와 별도로 분리
// ----------------------------------------------------------------------------

#[derive(Clone)]
pub struct HttpState {
    pub user_hub:       Arc<UserHub>,
    pub channel_hub:    Arc<ChannelHub>,
    pub media_peer_hub: Arc<MediaPeerHub>,
    /// 서버 프로세스 시작 시각 (Unix millis) — uptime 계산용
    pub start_time_ms:  u64,
}

impl HttpState {
    pub fn new(
        user_hub:       Arc<UserHub>,
        channel_hub:    Arc<ChannelHub>,
        media_peer_hub: Arc<MediaPeerHub>,
    ) -> Self {
        let start_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self { user_hub, channel_hub, media_peer_hub, start_time_ms }
    }
}

// ----------------------------------------------------------------------------
// [응답 타입] — 기존
// ----------------------------------------------------------------------------

/// GET /channels 응답 아이템
#[derive(Serialize)]
pub struct ChannelSummary {
    pub channel_id:    String,
    pub freq:          String,
    pub name:          String,
    pub member_count:  usize,
    pub capacity:      usize,
    pub created_at:    u64,
}

/// GET /channels/{id} 응답
#[derive(Serialize)]
pub struct ChannelDetail {
    pub channel_id:   String,
    pub freq:         String,
    pub name:         String,
    pub member_count: usize,
    pub capacity:     usize,
    pub created_at:   u64,
    pub peers:        Vec<PeerInfo>,
}

/// peer 정보 (채널 상세에 포함)
#[derive(Serialize)]
pub struct PeerInfo {
    pub user_id: String,
    pub ssrc:    u32,
}

// ----------------------------------------------------------------------------
// [응답 타입] — Admin
// ----------------------------------------------------------------------------

/// GET /admin/status
#[derive(Serialize)]
pub struct ServerStatus {
    pub uptime_secs:   u64,
    pub user_count:    usize,
    pub channel_count: usize,
    pub peer_count:    usize,
    pub floor_active:  usize, // Floor Taken 상태인 채널 수
}

/// GET /admin/users 아이템
#[derive(Serialize)]
pub struct AdminUserSummary {
    pub user_id:      String,
    pub priority:     u8,
    pub last_seen_ms: u64,   // Unix millis
    pub idle_secs:    u64,   // 마지막 heartbeat 이후 경과 초
}

/// GET /admin/users/{user_id}
#[derive(Serialize)]
pub struct AdminUserDetail {
    pub user_id:      String,
    pub priority:     u8,
    pub last_seen_ms: u64,
    pub idle_secs:    u64,
    pub channels:     Vec<String>, // 소속 채널 id 목록
}

/// GET /admin/channels 아이템 (Floor 상태 포함)
#[derive(Serialize)]
pub struct AdminChannelSummary {
    pub channel_id:    String,
    pub freq:          String,
    pub name:          String,
    pub member_count:  usize,
    pub capacity:      usize,
    pub floor_state:   String,          // "idle" | "taken"
    pub floor_holder:  Option<String>,  // Taken 시 holder user_id
    pub queue_len:     usize,
}

/// GET /admin/channels/{id}
#[derive(Serialize)]
pub struct AdminChannelDetail {
    pub channel_id:       String,
    pub freq:             String,
    pub name:             String,
    pub capacity:         usize,
    pub created_at:       u64,
    pub members:          Vec<String>,
    pub floor_state:      String,
    pub floor_holder:     Option<String>,
    pub floor_taken_secs: Option<u64>,  // Taken된 후 경과 초
    pub floor_priority:   u8,
    pub queue_len:        usize,
    pub queue:            Vec<AdminQueueEntry>,
    pub peers:            Vec<AdminPeerSummary>,
}

#[derive(Serialize)]
pub struct AdminQueueEntry {
    pub user_id:    String,
    pub priority:   u8,
    pub queued_at:  u64,
    pub wait_secs:  u64,
}

/// GET /admin/peers 아이템
#[derive(Serialize)]
pub struct AdminPeerSummary {
    pub ufrag:      String,
    pub user_id:    String,
    pub channel_id: String,
    pub address:    Option<String>,
    pub idle_secs:  u64,
    pub srtp_ready: bool,
}

/// GET /admin/peers/{ufrag}
#[derive(Serialize)]
pub struct AdminPeerDetail {
    pub ufrag:      String,
    pub user_id:    String,
    pub channel_id: String,
    pub address:    Option<String>,
    pub last_seen:  u64,
    pub idle_secs:  u64,
    pub srtp_ready: bool,
    pub tracks:     Vec<AdminTrack>,
}

#[derive(Serialize)]
pub struct AdminTrack {
    pub ssrc: u32,
    pub kind: String,
}

// ----------------------------------------------------------------------------
// [핸들러] — 기존
// ----------------------------------------------------------------------------

/// GET /channels
pub async fn list_channels(State(state): State<HttpState>) -> impl IntoResponse {
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
    list.sort_by(|a, b| a.freq.cmp(&b.freq));

    Json(list)
}

/// GET /channels/{id}
pub async fn get_channel(
    State(state): State<HttpState>,
    Path(channel_id): Path<String>,
) -> impl IntoResponse {
    let channel = match state.channel_hub.get(&channel_id) {
        Some(ch) => ch,
        None     => return (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("채널을 찾을 수 없습니다: {}", channel_id)
        }))).into_response(),
    };

    let peers: Vec<PeerInfo> = state.media_peer_hub
        .get_channel_endpoints(&channel_id)
        .into_iter()
        .map(|p| PeerInfo {
            user_id: p.user_id.clone(),
            ssrc: p.tracks.read().unwrap().first().map(|t| t.ssrc).unwrap_or(0),
        })
        .collect();

    let detail = ChannelDetail {
        channel_id:   channel.channel_id.clone(),
        freq:         channel.freq.clone(),
        name:         channel.name.clone(),
        member_count: channel.member_count(),
        capacity:     channel.capacity,
        created_at:   channel.created_at,
        peers,
    };

    Json(detail).into_response()
}

// ----------------------------------------------------------------------------
// [핸들러] — Admin 조회
// ----------------------------------------------------------------------------

/// GET /admin/status
pub async fn admin_status(State(state): State<HttpState>) -> impl IntoResponse {
    let now_ms       = current_timestamp();
    let uptime_secs  = now_ms.saturating_sub(state.start_time_ms) / 1000;

    let user_count    = state.user_hub.count();
    let channel_count = state.channel_hub.count();
    let peer_count    = state.media_peer_hub.count();
    let floor_active  = state.channel_hub.count_floor_taken();

    Json(ServerStatus { uptime_secs, user_count, channel_count, peer_count, floor_active })
}

/// GET /admin/users
pub async fn admin_list_users(State(state): State<HttpState>) -> impl IntoResponse {
    let now = current_timestamp();
    let mut list: Vec<AdminUserSummary> = state.user_hub
        .all_users()
        .into_iter()
        .map(|(uid, user)| {
            let last_seen_ms = user.last_seen.load(Ordering::Relaxed);
            AdminUserSummary {
                user_id:      uid,
                priority:     user.priority,
                last_seen_ms,
                idle_secs:    now.saturating_sub(last_seen_ms) / 1000,
            }
        })
        .collect();
    list.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    Json(list)
}

/// GET /admin/users/{user_id}
pub async fn admin_get_user(
    State(state): State<HttpState>,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    let user = match state.user_hub.get(&user_id) {
        Some(u) => u,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("User not found: {}", user_id)
        }))).into_response(),
    };

    let now          = current_timestamp();
    let last_seen_ms = user.last_seen.load(Ordering::Relaxed);

    // 소속 채널 목록 수집
    let channels: Vec<String> = state.channel_hub
        .channels.read().unwrap()
        .values()
        .filter(|ch| ch.get_members().contains(&user_id))
        .map(|ch| ch.channel_id.clone())
        .collect();

    Json(AdminUserDetail {
        user_id,
        priority: user.priority,
        last_seen_ms,
        idle_secs: now.saturating_sub(last_seen_ms) / 1000,
        channels,
    }).into_response()
}

/// GET /admin/channels
pub async fn admin_list_channels(State(state): State<HttpState>) -> impl IntoResponse {
    let channels = state.channel_hub.channels.read().unwrap();
    let mut list: Vec<AdminChannelSummary> = channels.values()
        .map(|ch| {
            let floor = ch.floor.lock().unwrap();
            AdminChannelSummary {
                channel_id:   ch.channel_id.clone(),
                freq:         ch.freq.clone(),
                name:         ch.name.clone(),
                member_count: ch.member_count(),
                capacity:     ch.capacity,
                floor_state:  floor_state_str(&floor.state),
                floor_holder: floor.floor_taken_by.clone(),
                queue_len:    floor.queue.len(),
            }
        })
        .collect();
    list.sort_by(|a, b| a.freq.cmp(&b.freq));
    Json(list)
}

/// GET /admin/channels/{channel_id}
pub async fn admin_get_channel(
    State(state): State<HttpState>,
    Path(channel_id): Path<String>,
) -> impl IntoResponse {
    let channel = match state.channel_hub.get(&channel_id) {
        Some(ch) => ch,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("Channel not found: {}", channel_id)
        }))).into_response(),
    };

    let now = current_timestamp();
    let (floor_state, floor_holder, floor_taken_secs, floor_priority, queue_len, queue_entries) = {
        let floor = channel.floor.lock().unwrap();
        let taken_secs = floor.floor_taken_at
            .map(|t| now.saturating_sub(t) / 1000);
        let entries: Vec<AdminQueueEntry> = floor.queue.iter()
            .map(|e| AdminQueueEntry {
                user_id:   e.user_id.clone(),
                priority:  e.priority,
                queued_at: e.queued_at,
                wait_secs: now.saturating_sub(e.queued_at) / 1000,
            })
            .collect();
        (
            floor_state_str(&floor.state),
            floor.floor_taken_by.clone(),
            taken_secs,
            floor.floor_priority,
            floor.queue.len(),
            entries,
        )
    };

    let members: Vec<String> = channel.get_members().into_iter().collect();

    let peers: Vec<AdminPeerSummary> = state.media_peer_hub
        .get_channel_endpoints(&channel_id)
        .into_iter()
        .map(|ep| {
            let last = ep.last_seen.load(Ordering::Relaxed);
            AdminPeerSummary {
                ufrag:      ep.ufrag.clone(),
                user_id:    ep.user_id.clone(),
                channel_id: ep.channel_id.clone(),
                address:    ep.get_address().map(|a| a.to_string()),
                idle_secs:  now.saturating_sub(last) / 1000,
                srtp_ready: ep.inbound_srtp.lock().unwrap().is_ready(),
            }
        })
        .collect();

    Json(AdminChannelDetail {
        channel_id: channel.channel_id.clone(),
        freq:       channel.freq.clone(),
        name:       channel.name.clone(),
        capacity:   channel.capacity,
        created_at: channel.created_at,
        members,
        floor_state,
        floor_holder,
        floor_taken_secs,
        floor_priority,
        queue_len,
        queue: queue_entries,
        peers,
    }).into_response()
}

/// GET /admin/peers
pub async fn admin_list_peers(State(state): State<HttpState>) -> impl IntoResponse {
    let now = current_timestamp();
    let mut list: Vec<AdminPeerSummary> = state.media_peer_hub
        .all_endpoints()
        .into_iter()
        .map(|ep| {
            let last = ep.last_seen.load(Ordering::Relaxed);
            AdminPeerSummary {
                ufrag:      ep.ufrag.clone(),
                user_id:    ep.user_id.clone(),
                channel_id: ep.channel_id.clone(),
                address:    ep.get_address().map(|a| a.to_string()),
                idle_secs:  now.saturating_sub(last) / 1000,
                srtp_ready: ep.inbound_srtp.lock().unwrap().is_ready(),
            }
        })
        .collect();
    list.sort_by(|a, b| a.user_id.cmp(&b.user_id));
    Json(list)
}

/// GET /admin/peers/{ufrag}
pub async fn admin_get_peer(
    State(state): State<HttpState>,
    Path(ufrag): Path<String>,
) -> impl IntoResponse {
    let ep = match state.media_peer_hub.get_by_ufrag(&ufrag) {
        Some(e) => e,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("Peer not found: {}", ufrag)
        }))).into_response(),
    };

    let now       = current_timestamp();
    let last       = ep.last_seen.load(Ordering::Relaxed);
    let srtp_ready = ep.inbound_srtp.lock().unwrap().is_ready();
    let tracks: Vec<AdminTrack> = ep.tracks.read().unwrap()
        .iter()
        .map(|t| AdminTrack {
            ssrc: t.ssrc,
            kind: format!("{:?}", t.kind).to_lowercase(),
        })
        .collect();

    Json(AdminPeerDetail {
        ufrag:      ep.ufrag.clone(),
        user_id:    ep.user_id.clone(),
        channel_id: ep.channel_id.clone(),
        address:    ep.get_address().map(|a| a.to_string()),
        last_seen:  last,
        idle_secs:  now.saturating_sub(last) / 1000,
        srtp_ready,
        tracks,
    }).into_response()
}

// ----------------------------------------------------------------------------
// [핸들러] — Admin 조작
// ----------------------------------------------------------------------------

/// POST /admin/floor-revoke/{channel_id}
/// Floor를 강제로 Idle로 되돌린다 (holder + queue 모두 초기화)
pub async fn admin_floor_revoke(
    State(state): State<HttpState>,
    Path(channel_id): Path<String>,
) -> impl IntoResponse {
    let channel = match state.channel_hub.get(&channel_id) {
        Some(ch) => ch,
        None => return (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("Channel not found: {}", channel_id)
        }))).into_response(),
    };

    let (was_taken, holder) = {
        let floor = channel.floor.lock().unwrap();
        (
            floor.state == FloorControlState::Taken,
            floor.floor_taken_by.clone(),
        )
    };

    if !was_taken {
        return (StatusCode::CONFLICT, Json(serde_json::json!({
            "error": "Floor is already idle",
            "channel_id": channel_id
        }))).into_response();
    }

    // Floor 강제 초기화 (멤버에게 알리는 시그널링은 admin 범위 밖)
    {
        let mut floor = channel.floor.lock().unwrap();
        floor.queue.clear();
        floor.clear_taken();
    }

    tracing::warn!("[admin] floor-revoke channel={} was_held_by={:?}", channel_id, holder);

    Json(serde_json::json!({
        "ok": true,
        "channel_id": channel_id,
        "revoked_from": holder
    })).into_response()
}

// ----------------------------------------------------------------------------
// [유틸]
// ----------------------------------------------------------------------------

fn floor_state_str(state: &FloorControlState) -> String {
    match state {
        FloorControlState::Idle  => "idle".to_string(),
        FloorControlState::Taken => "taken".to_string(),
    }
}
