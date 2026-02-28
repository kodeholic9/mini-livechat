// author: kodeholic (powered by Claude)
// Admin REST API 핸들러
//
// 조회
//   GET /admin/status                  → 서버 상태 요약
//   GET /admin/users                   → User 전체 목록
//   GET /admin/users/{user_id}         → User 상세
//   GET /admin/channels                → Channel 전체 목록 (Floor 상태 포함)
//   GET /admin/channels/{channel_id}   → Channel 상세
//   GET /admin/peers                   → Endpoint 전체 목록
//   GET /admin/peers/{ufrag}           → Endpoint 상세
//
// 조작
//   POST /admin/floor-revoke/{channel_id} → Floor 강제 revoke

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::sync::atomic::Ordering;

use crate::core::FloorControlState;
use crate::protocol::message::{FloorIdlePayload, FloorRevokePayload, GatewayPacket};
use crate::protocol::opcode::server;
use crate::utils::current_timestamp;

use super::dto::*;
use super::state::HttpState;

// ----------------------------------------------------------------------------
// [유틸]
// ----------------------------------------------------------------------------

fn floor_state_str(state: &FloorControlState) -> String {
    match state {
        FloorControlState::Idle  => "idle".to_string(),
        FloorControlState::Taken => "taken".to_string(),
    }
}

// ----------------------------------------------------------------------------
// [핸들러]
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

/// POST /admin/floor-revoke/{channel_id}
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

    // Floor 강제 초기화
    {
        let mut floor = channel.floor.lock().unwrap();
        floor.queue.clear();
        floor.clear_taken();
    }

    tracing::warn!("[admin] floor-revoke channel={} was_held_by={:?}", channel_id, holder);

    // holder에게 FLOOR_REVOKE 전송
    if let Some(ref holder_id) = holder {
        let revoke_json = serde_json::to_string(&GatewayPacket::new(
            server::FLOOR_REVOKE,
            FloorRevokePayload {
                channel_id: channel_id.clone(),
                cause: "admin_revoke".to_string(),
            },
        )).unwrap_or_default();
        if let Some(user) = state.user_hub.get(holder_id) {
            let _ = user.tx.send(revoke_json).await;
        }
    }

    // 전체 멤버에게 FLOOR_IDLE 전송
    let idle_json = serde_json::to_string(&GatewayPacket::new(
        server::FLOOR_IDLE,
        FloorIdlePayload {
            channel_id: channel_id.clone(),
        },
    )).unwrap_or_default();
    let members = channel.get_members();
    state.user_hub.broadcast_to(&members, &idle_json, None).await;

    Json(serde_json::json!({
        "ok": true,
        "channel_id": channel_id,
        "revoked_from": holder
    })).into_response()
}
