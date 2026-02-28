// author: kodeholic (powered by Claude)
// 매직 넘버를 배제하고 시스템 전체의 성능과 한계를 제어하는 상수 모음입니다.

/// 미디어 패킷 수신용 단일 UDP 포트
pub const SERVER_UDP_PORT: u16 = 10000;

/// 채널당 최대 수용 인원 (메모리 OOM 방어)
pub const MAX_PEERS_PER_CHANNEL: usize = 100;

/// 송신(Egress) 워커 큐 사이즈.
/// 꽉 차면 지연 발생 방지를 위해 오래된 패킷을 버립니다(Drop/Backpressure).
pub const EGRESS_QUEUE_SIZE: usize = 2048;

/// 좀비 세션 reaper 실행 주기 (10초)
pub const REAPER_INTERVAL_MS: u64 = 10_000;

/// DTLS 핸드셰이크 최대 허용 시간 (10초)
pub const DTLS_HANDSHAKE_TIMEOUT_MS: u64 = 10_000;

/// 연결이 끊긴 좀비 세션을 정리하기 위한 타임아웃 (30초)
pub const ZOMBIE_TIMEOUT_MS: u64 = 30_000;

/// 웹소켓 시그널링 서버 TCP 포트
pub const SIGNALING_PORT: u16 = 8080;

/// 클라이언트가 HEARTBEAT를 보내야 하는 주기 (밀리초)
pub const HEARTBEAT_INTERVAL_MS: u64 = 30_000;

/// 채팅 메시지 최대 길이 (bytes)
pub const MAX_MESSAGE_LENGTH: usize = 2_000;

/// IDENTIFY 토큰 검증용 Secret Key
/// 운영 환경에서는 환경변수 LIVECHAT_SECRET 으로 오버라이드 할 것
pub const DEFAULT_SECRET_KEY: &str = "changeme-secret";

// ----------------------------------------------------------------------------
// Floor Control (MBCP TS 24.380 기반)
// ----------------------------------------------------------------------------

/// 클라이언트 Floor Ping 수신 타임아웃 — 이 시간 안에 Ping 없으면 Floor Revoke (6초)
/// 클라이언트 송신 주기 2초 기준 — 네트워크 지연마지 감안하여 3배 여유 확보
pub const FLOOR_PING_TIMEOUT_MS: u64 = 6_000;

/// 최대 발언 점유 시간 — Emergency 포함 무조건 Revoke (30초)
pub const FLOOR_MAX_TAKEN_MS: u64 = 30_000;

/// Floor Request 응답 대기 타이머 T101 (3초)
pub const FLOOR_T101_MS: u64 = 3_000;

/// Floor Release 응답 대기 타이머 T100 (3초)
pub const FLOOR_T100_MS: u64 = 3_000;

/// 발언권 우선순위 — Emergency 고정값 (최고)
pub const FLOOR_PRIORITY_EMERGENCY: u8 = 255;

/// 발언권 우선순위 — Imminent Peril 고정값
pub const FLOOR_PRIORITY_IMMINENT_PERIL: u8 = 200;

/// 발언권 우선순위 — 일반 기본값
pub const FLOOR_PRIORITY_DEFAULT: u8 = 100;

// ----------------------------------------------------------------------------
// 사전 생성 채널 (서버 시작 시 자동 생성)
// (channel_id, freq, name, capacity)
// ----------------------------------------------------------------------------
pub const PRESET_CHANNELS: &[(&str, &str, &str, usize)] = &[
    ("CH_0001", "0001", "📢 영업/시연",  20),
    ("CH_0002", "0002", "🤝 스스 파트너스", 20),
    ("CH_0003", "0003", "🏠 동천 패밀리",  20),
];
