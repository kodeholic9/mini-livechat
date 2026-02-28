// author: kodeholic (powered by Claude)

pub mod floor;
pub mod message;
pub mod opcode;
pub mod protocol;
pub mod sdp;

pub use protocol::{ws_handler, AppState};

// ----------------------------------------------------------------------------
// 광고 IP 전역 저장소
// run_udp_relay() 시작 시 1회 세팅, 이후 build_sdp_answer()에서 읽음
// OnceLock: 실수로 두 번 써져도 첫 번 값만 유지
// ----------------------------------------------------------------------------
use std::sync::OnceLock;
static ADVERTISE_IP: OnceLock<String> = OnceLock::new();

pub fn set_advertise_ip(ip: String) {
    let _ = ADVERTISE_IP.set(ip); // 이미 세팅된 경우 무시
}

pub fn get_advertise_ip() -> String {
    ADVERTISE_IP
        .get()
        .cloned()
        .unwrap_or_else(|| sdp::detect_local_ip()) // fallback
}
