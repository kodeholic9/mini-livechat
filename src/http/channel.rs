// author: kodeholic (powered by Claude)
// 일반 채널 조회 핸들러
//   GET /channels          → 채널 목록
//   GET /channels/{id}     → 채널 상세 + peer 목록

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};

use super::dto::{ChannelDetail, ChannelSummary, PeerInfo};
use super::state::HttpState;

/// GET /channels
pub async fn list_channels(State(state): State<HttpState>) -> impl IntoResponse {
    let channels = state.channel_hub.channels.read().unwrap();

    let mut list: Vec<ChannelSummary> = channels.values()
        .map(|ch| ChannelSummary {
            channel_id:   ch.channel_id.clone(),
            freq:         ch.freq.clone(),
            name:         ch.name.clone(),
            mode:         ch.mode.to_string(),
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
        mode:         channel.mode.to_string(),
        member_count: channel.member_count(),
        capacity:     channel.capacity,
        created_at:   channel.created_at,
        peers,
    };

    Json(detail).into_response()
}
