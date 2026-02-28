// author: kodeholic (powered by Claude)
// HttpState — HTTP 핸들러 공유 상태

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{ChannelHub, MediaPeerHub, UserHub};
use crate::trace::TraceHub;

#[derive(Clone)]
pub struct HttpState {
    pub user_hub:       Arc<UserHub>,
    pub channel_hub:    Arc<ChannelHub>,
    pub media_peer_hub: Arc<MediaPeerHub>,
    pub trace_hub:      Arc<TraceHub>,
    /// 서버 프로세스 시작 시각 (Unix millis) — uptime 계산용
    pub start_time_ms:  u64,
}

impl HttpState {
    pub fn new(
        user_hub:       Arc<UserHub>,
        channel_hub:    Arc<ChannelHub>,
        media_peer_hub: Arc<MediaPeerHub>,
        trace_hub:      Arc<TraceHub>,
    ) -> Self {
        let start_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        Self { user_hub, channel_hub, media_peer_hub, trace_hub, start_time_ms }
    }
}
