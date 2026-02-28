// author: kodeholic (powered by Claude)

pub mod config;
pub mod core;
pub mod error;
pub mod http;
pub mod media;
pub mod protocol;
pub mod trace;
pub mod utils;

use axum::{routing::{get, post}, Router};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

use crate::core::{ChannelHub, MediaPeerHub, UserHub};
use crate::media::{DtlsSessionMap, ServerCert};
use crate::protocol::{ws_handler, AppState};
use crate::http::HttpState;
use crate::trace::TraceHub;

/// CLI에서 주입되는 런타임 설정
/// - 기본값은 config.rs 상수
/// - 비밀값(SECRET)은 환경변수로 별도 관리
pub struct ServerArgs {
    pub port:         u16,
    pub udp_port:     u16,
    pub advertise_ip: Option<String>, // None이면 detect_local_ip() 자동 감지
}

pub async fn run_server(args: ServerArgs) {
    let user_hub       = Arc::new(UserHub::new());
    let channel_hub    = Arc::new(ChannelHub::new());
    let media_peer_hub = Arc::new(MediaPeerHub::new());

    // 사전 정의 채널 5개 생성
    for (channel_id, freq, name, capacity) in config::PRESET_CHANNELS {
        channel_hub.create(channel_id, freq, name, *capacity);
        info!("[channel] preset created: {} freq={} name={} cap={}", channel_id, freq, name, capacity);
    }

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

    let trace_hub = TraceHub::new();

    let app_state = AppState {
        user_hub:       Arc::clone(&user_hub),
        channel_hub:    Arc::clone(&channel_hub),
        media_peer_hub: Arc::clone(&media_peer_hub),
        server_cert:    Arc::clone(&server_cert),
        trace_hub:      Arc::clone(&trace_hub),
        udp_port:       args.udp_port,
    };

    // UDP 미디어 릴레이 태스크
    tokio::spawn(media::run_udp_relay(
        Arc::clone(&media_peer_hub),
        Arc::clone(&channel_hub),
        Arc::clone(&server_cert),
        Arc::clone(&dtls_session_map),
        args.udp_port,
        args.advertise_ip.clone(),
    ));

    // 좀비 세션 자동 종료 태스크 (Floor 타임아웃 체크 포함)
    tokio::spawn(run_zombie_reaper(
        Arc::clone(&user_hub),
        Arc::clone(&channel_hub),
        Arc::clone(&media_peer_hub),
        Arc::clone(&dtls_session_map),
        Arc::clone(&trace_hub),
    ));

    let http_state = HttpState::new(
        Arc::clone(&user_hub),
        Arc::clone(&channel_hub),
        Arc::clone(&media_peer_hub),
        Arc::clone(&trace_hub),
    );

    let admin_router = Router::new()
        .route("/admin/status",                 get(http::admin_status))
        .route("/admin/users",                  get(http::admin_list_users))
        .route("/admin/users/{user_id}",        get(http::admin_get_user))
        .route("/admin/channels",               get(http::admin_list_channels))
        .route("/admin/channels/{channel_id}",  get(http::admin_get_channel))
        .route("/admin/peers",                  get(http::admin_list_peers))
        .route("/admin/peers/{ufrag}",          get(http::admin_get_peer))
        .route("/admin/floor-revoke/{channel_id}", post(http::admin_floor_revoke))
        .route("/trace",             get(http::trace_stream))
        .route("/trace/{channel_id}", get(http::trace_stream))
        .route("/channels",      get(http::list_channels))
        .route("/channels/{id}", get(http::get_channel))
        .with_state(http_state);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state)
        .merge(admin_router);

    let addr     = format!("0.0.0.0:{}", args.port);
    let listener = TcpListener::bind(&addr).await.unwrap();

    info!("[mini-livechat] Signaling Server on ws://{}", addr);
    info!("[mini-livechat] UDP Media Relay on port {}", args.udp_port);
    info!("[mini-livechat] DTLS fingerprint: {}", server_cert.fingerprint);
    if let Some(ref ip) = args.advertise_ip {
        info!("[mini-livechat] Advertise IP: {} (manual)", ip);
    } else {
        info!("[mini-livechat] Advertise IP: auto detect");
    }

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
    trace_hub:    Arc<TraceHub>,
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

        // 4. Floor 타임아웃 체크 (ping_timeout / max_duration Revoke)
        crate::protocol::floor::check_floor_timeouts_traced(&user_hub, &channel_hub, &trace_hub).await;

        let total = dead_users.len() + dead_peers.len() + stale.len();
        if total > 0 {
            info!("[zombie-reaper] Cleaned {} user(s), {} peer(s), {} dtls session(s)",
                dead_users.len(), dead_peers.len(), stale.len());
        }
    }
}
