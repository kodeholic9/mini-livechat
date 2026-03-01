// author: kodeholic (powered by Claude)

use clap::Parser;
use mini_livechat::{run_server, ServerArgs};

/// mini-livechat 미디어 릴레이 서버
#[derive(Parser, Debug)]
#[command(author, version, about)]
pub struct Args {
    /// WebSocket 시그널링 포트
    #[arg(long, default_value_t = mini_livechat::config::SIGNALING_PORT)]
    pub port: u16,

    /// UDP 미디어 릴레이 포트
    #[arg(long, default_value_t = mini_livechat::config::SERVER_UDP_PORT)]
    pub udp_port: u16,

    /// SDP candidate에 광고할 IP (생략 시 라우팅 테이블 기반 자동 감지)
    #[arg(long)]
    pub advertise_ip: Option<String>,
}

#[tokio::main]
async fn main() {
    // 환경 변수 기반 로깅 초기화 (RUST_LOG=trace 등으로 제어)
    // 로컬 시각 타이머 — chrono::Local 기반
    struct LocalTimer;
    impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
        fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
            write!(w, "{}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"))
        }
    }
    tracing_subscriber::fmt()
        .with_timer(LocalTimer)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    // advertise_ip: CLI 인자 > 환경변수 ADVERTISE_IP > None(자동 감지)
    let advertise_ip = args.advertise_ip
        .or_else(|| std::env::var("ADVERTISE_IP").ok().filter(|s| !s.is_empty()));

    run_server(ServerArgs {
        port:         args.port,
        udp_port:     args.udp_port,
        advertise_ip,
    })
    .await;
}
