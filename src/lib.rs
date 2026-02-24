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
use tracing::{error, info};

use crate::core::{ChannelHub, MediaPeerHub, UserHub};
use crate::media::{DtlsSessionMap, ServerCert};
use crate::protocol::{ws_handler, AppState};

pub async fn run_server() {
    let user_hub       = Arc::new(UserHub::new());
    let channel_hub    = Arc::new(ChannelHub::new());
    let media_peer_hub = Arc::new(MediaPeerHub::new());

    // DTLS 자체서명 인증서 — 프로세스 시작 시 1회 생성, 전체 공유
    let server_cert = match ServerCert::generate() {
        Ok(c)  => Arc::new(c),
        Err(e) => {
            error!("[dtls] Failed to generate server certificate: {}", e);
            return;
        }
    };

    // DTLS 핸드셰이크 세션 맵 (SocketAddr → 패킷 주입 채널)
    let dtls_session_map = Arc::new(DtlsSessionMap::new());

    let app_state = AppState {
        user_hub:       Arc::clone(&user_hub),
        channel_hub:    Arc::clone(&channel_hub),
        media_peer_hub: Arc::clone(&media_peer_hub),
    };

    // UDP 미디어 릴레이 태스크
    tokio::spawn(media::run_udp_relay(
        Arc::clone(&media_peer_hub),
        Arc::clone(&server_cert),
        Arc::clone(&dtls_session_map),
    ));

    // 좀비 세션 자동 종료 태스크
    tokio::spawn(run_zombie_reaper(Arc::clone(&user_hub), Arc::clone(&media_peer_hub)));

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state);

    let addr     = format!("0.0.0.0:{}", config::SIGNALING_PORT);
    let listener = TcpListener::bind(&addr).await.unwrap();

    info!("[mini-livechat] Signaling Server on ws://{}", addr);
    info!("[mini-livechat] UDP Media Relay on port {}", config::SERVER_UDP_PORT);
    info!("[mini-livechat] DTLS fingerprint: {}", server_cert.fingerprint);

    axum::serve(listener, app).await.unwrap();
}

/// 좀비 세션 자동 종료 태스크
async fn run_zombie_reaper(user_hub: Arc<UserHub>, media_peer_hub: Arc<MediaPeerHub>) {
    let interval  = tokio::time::Duration::from_millis(config::HEARTBEAT_INTERVAL_MS);
    let mut timer = tokio::time::interval(interval);
    timer.tick().await; // 첫 틱 skip

    info!("[zombie-reaper] Started (interval={}ms, timeout={}ms)",
        config::HEARTBEAT_INTERVAL_MS, config::ZOMBIE_TIMEOUT_MS);

    loop {
        timer.tick().await;

        let dead_users = user_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for uid in &dead_users {
            user_hub.unregister(uid);
            info!("[zombie-reaper] Removed zombie user: {}", uid);
        }

        let dead_peers = media_peer_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for ufrag in &dead_peers {
            media_peer_hub.remove(ufrag);
            info!("[zombie-reaper] Removed zombie peer: ufrag={}", ufrag);
        }

        if !dead_users.is_empty() || !dead_peers.is_empty() {
            info!("[zombie-reaper] Cleaned {} user(s), {} peer(s)",
                dead_users.len(), dead_peers.len());
        }
    }
}
