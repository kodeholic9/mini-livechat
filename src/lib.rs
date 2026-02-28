// author: kodeholic (powered by Claude)

pub mod config;
pub mod core;
pub mod error;
pub mod http;
pub mod media;
pub mod protocol;
pub mod reaper;
pub mod trace;
pub mod utils;

use axum::{routing::{get, post}, Router};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
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

    // 사전 정의 채널 생성
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
    tokio::spawn(reaper::run_zombie_reaper(
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

    // CORS — 개발/운영 모두 전체 허용 (Admin 대시보드, PTT 클라이언트 로컬 접속)
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state)
        .merge(admin_router)
        .layer(cors);

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
