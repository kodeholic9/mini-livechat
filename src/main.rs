// author: kodeholic (powered by Gemini)

use mini_livechat::run_server;

#[tokio::main]
async fn main() {
    // 환경 변수 기반 로깅 초기화 (기본값: info)
    tracing_subscriber::fmt::init();
    run_server().await;
}