// author: kodeholic (powered by Claude)
// 매직 넘버를 배제하고 시스템 전체의 성능과 한계를 제어하는 상수 모음입니다.

/// 미디어 패킷 수신용 단일 UDP 포트
pub const SERVER_UDP_PORT: u16 = 10000;

/// 채널당 최대 수용 인원 (메모리 OOM 방어)
pub const MAX_PEERS_PER_CHANNEL: usize = 100;

/// 송신(Egress) 워커 큐 사이즈.
/// 꽉 차면 지연 발생 방지를 위해 오래된 패킷을 버립니다(Drop/Backpressure).
pub const EGRESS_QUEUE_SIZE: usize = 2048;

/// 연결이 끊긴 좀비 세션을 정리하기 위한 타임아웃 (30초)
pub const ZOMBIE_TIMEOUT_MS: u64 = 30_000;

/// 웹소켓 시그널링 서버 TCP 포트
pub const SIGNALING_PORT: u16 = 8080;

/// 클라이언트가 HEARTBEAT를 보내야 하는 주기 (밀리초)
pub const HEARTBEAT_INTERVAL_MS: u64 = 30_000;

/// 채팅 메시지 최대 길이 (bytes)
pub const MAX_MESSAGE_LENGTH: usize = 2_000;
