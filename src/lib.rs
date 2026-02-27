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
        server_cert:    Arc::clone(&server_cert),
    };

    // UDP 미디어 릴레이 태스크
    tokio::spawn(media::run_udp_relay(
        Arc::clone(&media_peer_hub),
        Arc::clone(&server_cert),
        Arc::clone(&dtls_session_map),
    ));

    // Floor Ping 태스크 (Floor Taken 상태에서 holder 생존 확인)
    tokio::spawn(crate::protocol::floor::run_floor_ping_task(
        Arc::clone(&user_hub),
        Arc::clone(&channel_hub),
    ));

    // 좀비 세션 자동 종료 태스크
    tokio::spawn(run_zombie_reaper(
        Arc::clone(&user_hub),
        Arc::clone(&channel_hub),
        Arc::clone(&media_peer_hub),
        Arc::clone(&dtls_session_map),
    ));

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
///
/// 주기마다 수행:
///   1. 좋뽐 User (WS 하트비트 없음) 제거 + 소속 체널 멤버에서 제외
///   2. 좋뽐 Endpoint (진 패킷 없음) 제거
///   3. 단절된 DTLS 핸드셰이크 세션 제거 (tx 닫힌 세션 정리)
async fn run_zombie_reaper(
    user_hub:     Arc<UserHub>,
    channel_hub:  Arc<ChannelHub>,
    media_hub:    Arc<MediaPeerHub>,
    session_map:  Arc<media::DtlsSessionMap>,
) {
    let interval  = tokio::time::Duration::from_millis(config::REAPER_INTERVAL_MS);
    let mut timer = tokio::time::interval(interval);
    timer.tick().await; // 첫 틱 skip (startup 시 즉시 실행 방지)

    info!("[zombie-reaper] Started (interval={}ms, timeout={}ms)",
        config::REAPER_INTERVAL_MS, config::ZOMBIE_TIMEOUT_MS);

    loop {
        timer.tick().await;

        // 1. 좋뽐 User 정리
        //    WS 하트비트가 ZOMBIE_TIMEOUT_MS 동안 없으면 제거
        //    + 대상 유저가 소속된 모든 체널 멤버에서 제외
        let dead_users = user_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for uid in &dead_users {
            // 체널 멤버에서 먼저 제거 (체널 유지)
            let channels = channel_hub.channels.read().unwrap();
            for ch in channels.values() {
                ch.remove_member(uid);
            }
            drop(channels);
            user_hub.unregister(uid);
            info!("[zombie-reaper] user={} removed (no heartbeat)", uid);
        }

        // 2. 좋뽐 Endpoint 정리
        //    UDP 패킷이 ZOMBIE_TIMEOUT_MS 동안 없으면 제거
        let dead_peers = media_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for ufrag in &dead_peers {
            media_hub.remove(ufrag);
            info!("[zombie-reaper] peer ufrag={} removed (no media)", ufrag);
        }

        // 3. 단절된 DTLS 세션 정리
        //    tx가 닫힌 세션 = 핸드셰이크 태스크가 종료됐거나 타임아웃
        let stale = session_map.remove_stale().await;
        for addr in &stale {
            info!("[zombie-reaper] dtls session stale addr={}", addr);
        }

        let total = dead_users.len() + dead_peers.len() + stale.len();
        if total > 0 {
            info!("[zombie-reaper] Cleaned {} user(s), {} peer(s), {} dtls session(s)",
                dead_users.len(), dead_peers.len(), stale.len());
        }
    }
}
