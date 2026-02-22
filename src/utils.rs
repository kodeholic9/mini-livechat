// author: kodeholic (powered by Gemini)

use std::time::{SystemTime, UNIX_EPOCH};

/// 현재 시간을 밀리초 단위의 Unix Timestamp로 반환합니다.
/// 에러 발생 시 시스템 패닉 대신 0(기본값)을 반환하여 장애를 방어합니다.
pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}