// author: kodeholic (powered by Claude)
//
// HTTP REST API 핸들러
//
// GET /channels          → 채널 목록 (id, member_count, capacity)
// GET /channels/{id}     → 채널 상세 + peer 목록 (user_id, ssrc)

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::Serialize;
use std::sync::Arc;

use crate::core::{ChannelHub, MediaPeerHub};

// ----------------------------------------------------------------------------
// [공유 상태] HTTP 핸들러용 — WS AppState와 별도로 분리
// ----------------------------------------------------------------------------

#[derive(Clone)]
pub struct HttpState {
    pub channel_hub:    Arc<ChannelHub>,
    pub media_peer_hub: Arc<MediaPeerHub>,
}

// ----------------------------------------------------------------------------
// [응답 타입]
// ----------------------------------------------------------------------------

/// GET /channels 응답 아이템
#[derive(Serialize)]
pub struct ChannelSummary {
    pub channel_id:    String,
    pub member_count:  usize,
    pub capacity:      usize,
    pub created_at:    u64,
}

/// GET /channels/{id} 응답
#[derive(Serialize)]
pub struct ChannelDetail {
    pub channel_id:   String,
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
// [핸들러]
// ----------------------------------------------------------------------------

/// GET /channels
/// 현재 존재하는 모든 채널의 요약 목록 반환
pub async fn list_channels(State(state): State<HttpState>) -> impl IntoResponse {
    let channels = state.channel_hub.channels.read().unwrap();

    let list: Vec<ChannelSummary> = channels.values()
        .map(|ch| ChannelSummary {
            channel_id:   ch.channel_id.clone(),
            member_count: ch.member_count(),
            capacity:     ch.capacity,
            created_at:   ch.created_at,
        })
        .collect();

    Json(list)
}

/// GET /channels/{id}
/// 채널 상세 정보 + 현재 접속 중인 peer 목록 반환
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

    // MediaPeerHub에서 해당 채널 peer 목록 조회
    let peers: Vec<PeerInfo> = state.media_peer_hub
        .get_channel_peers(&channel_id)
        .into_iter()
        .map(|p| PeerInfo { user_id: p.user_id.clone(), ssrc: p.ssrc })
        .collect();

    let detail = ChannelDetail {
        channel_id:   channel.channel_id.clone(),
        member_count: channel.member_count(),
        capacity:     channel.capacity,
        created_at:   channel.created_at,
        peers,
    };

    Json(detail).into_response()
}
