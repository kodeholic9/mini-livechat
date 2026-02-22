// author: kodeholic (powered by Gemini)

pub mod config;
pub mod utils;
pub mod error;
pub mod core;
pub mod signaling;

use axum::{routing::get, Router};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

use crate::core::{LiveChannelHub, LivePeerHub};
use crate::signaling::{ws_handler, AppState};

/// 미디어 릴레이 서버 엔진 구동 (main.rs에서 호출)
pub async fn run_server() {
    // 1. 코어 상태(Hub) 메모리 초기화
    // 시스템 전체에서 단 하나만 존재하며, 모든 워커가 이 Arc 포인터를 공유합니다.
    let peer_hub = Arc::new(LivePeerHub::new());
    let channel_hub = Arc::new(LiveChannelHub::new());

    // 2. Axum 프레임워크에 주입할 공유 상태 조립
    let app_state = AppState {
        peer_hub: Arc::clone(&peer_hub),
        channel_hub: Arc::clone(&channel_hub),
    };

    // 3. 웹소켓 라우터 구성
    // 클라이언트가 "ws://서버IP:8080/ws" 경로로 들어오면 ws_handler로 토스합니다.
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state);

    // 4. 시그널링 서버(TCP) 바인딩 및 구동
    let addr = format!("0.0.0.0:{}", config::SIGNALING_PORT);
    let listener = TcpListener::bind(&addr).await.unwrap();
    
    info!("[mini-livechat] Signaling Server is running on ws://{}", addr);
    info!("[mini-livechat] Ready to accept UDP media packets on port {}", config::SERVER_UDP_PORT);

    // 서버 무한 루프 시작 (여기서 블로킹되며 클라이언트 요청을 대기합니다)
    axum::serve(listener, app).await.unwrap();
}