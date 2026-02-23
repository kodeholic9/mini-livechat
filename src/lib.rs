// author: kodeholic (powered by Claude)

pub mod config;
pub mod core;
pub mod error;
pub mod media;
pub mod protocol;
pub mod utils;

use axum::{routing::get, Router};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

use crate::core::{ChannelHub, MediaPeerHub, UserHub};
use crate::protocol::{ws_handler, AppState};

pub async fn run_server() {
    let user_hub       = Arc::new(UserHub::new());
    let channel_hub    = Arc::new(ChannelHub::new());
    let media_peer_hub = Arc::new(MediaPeerHub::new());

    let app_state = AppState {
        user_hub:       Arc::clone(&user_hub),
        channel_hub:    Arc::clone(&channel_hub),
        media_peer_hub: Arc::clone(&media_peer_hub),
    };

    // UDP 미디어 릴레이 태스크
    tokio::spawn(media::run_udp_relay(Arc::clone(&media_peer_hub)));

    // 좀비 세션 자동 종료 태스크
    tokio::spawn(run_zombie_reaper(Arc::clone(&user_hub), Arc::clone(&media_peer_hub)));

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state);

    let addr     = format!("0.0.0.0:{}", config::SIGNALING_PORT);
    let listener = TcpListener::bind(&addr).await.unwrap();

    info!("[mini-livechat] Signaling Server running on ws://{}", addr);
    info!("[mini-livechat] UDP Media Relay running on port {}", config::SERVER_UDP_PORT);

    axum::serve(listener, app).await.unwrap();
}

/// 좀비 세션 자동 종료 태스크
/// - ZOMBIE_TIMEOUT_MS 동안 heartbeat 없는 유저 → UserHub 에서 제거
/// - ZOMBIE_TIMEOUT_MS 동안 UDP 패킷 없는 MediaPeer → MediaPeerHub 에서 제거
/// - HEARTBEAT_INTERVAL_MS 주기로 순회 (타임아웃의 절반 수준)
async fn run_zombie_reaper(user_hub: Arc<UserHub>, media_peer_hub: Arc<MediaPeerHub>) {
    let interval  = tokio::time::Duration::from_millis(config::HEARTBEAT_INTERVAL_MS);
    let mut timer = tokio::time::interval(interval);
    timer.tick().await; // 첫 틱은 즉시 발생하므로 skip

    info!("[zombie-reaper] Started (interval={}ms, timeout={}ms)",
        config::HEARTBEAT_INTERVAL_MS, config::ZOMBIE_TIMEOUT_MS);

    loop {
        timer.tick().await;

        // 좀비 WS 세션 정리
        let dead_users = user_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for user_id in &dead_users {
            user_hub.unregister(user_id);
            info!("[zombie-reaper] Removed zombie user: {}", user_id);
        }

        // 좀비 MediaPeer 정리
        let dead_peers = media_peer_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for ssrc in &dead_peers {
            media_peer_hub.remove(*ssrc);
            info!("[zombie-reaper] Removed zombie peer: ssrc={}", ssrc);
        }

        if !dead_users.is_empty() || !dead_peers.is_empty() {
            info!("[zombie-reaper] Cleaned {} user(s), {} peer(s)",
                dead_users.len(), dead_peers.len());
        }
    }
}
