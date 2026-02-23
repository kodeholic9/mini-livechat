// author: kodeholic (powered by Claude)

pub mod config;
pub mod core;
pub mod error;
pub mod protocol;
pub mod utils;

use axum::{routing::get, Router};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::info;

use crate::core::{ChannelHub, MediaPeerHub, UserHub};
use crate::protocol::{ws_handler, AppState};

pub async fn run_server() {
    let app_state = AppState {
        user_hub:       Arc::new(UserHub::new()),
        channel_hub:    Arc::new(ChannelHub::new()),
        media_peer_hub: Arc::new(MediaPeerHub::new()),
    };

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(app_state);

    let addr     = format!("0.0.0.0:{}", config::SIGNALING_PORT);
    let listener = TcpListener::bind(&addr).await.unwrap();

    info!("[mini-livechat] Signaling Server is running on ws://{}", addr);
    info!("[mini-livechat] Ready to accept UDP media packets on port {}", config::SERVER_UDP_PORT);

    axum::serve(listener, app).await.unwrap();
}
