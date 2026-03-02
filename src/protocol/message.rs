// author: kodeholic (powered by Claude)

use serde::{Deserialize, Serialize};

// ----------------------------------------------------------------------------
// [공통] Gateway 패킷 봉투 (Envelope)
// ----------------------------------------------------------------------------

/// 모든 WebSocket 메시지의 최상위 구조체
#[derive(Serialize, Deserialize, Debug)]
pub struct GatewayPacket {
    pub op: u8,
    pub d: Option<serde_json::Value>,
}

// ----------------------------------------------------------------------------
// [C→S] 클라이언트 요청 payload 타입들
// ----------------------------------------------------------------------------

/// op: IDENTIFY (3)
#[derive(Deserialize, Debug)]
pub struct IdentifyPayload {
    pub user_id:  String,
    pub token:    String,
    pub priority: Option<u8>,  // Floor Control 우선순위 (없으면 FLOOR_PRIORITY_DEFAULT)
}

/// op: CHANNEL_CREATE (10)
#[derive(Deserialize, Debug)]
pub struct ChannelCreatePayload {
    pub channel_id:   String,
    pub freq:         String,   // 주파수번호 4자리
    pub channel_name: String,
    pub mode:         Option<String>,  // "ptt" | "conference" (없으면 기본 ptt)
}

/// op: CHANNEL_JOIN (11)
#[derive(Deserialize, Debug)]
pub struct ChannelJoinPayload {
    pub channel_id: String,
    pub ssrc:       u32,
    pub ufrag:      String,
    pub sdp_offer:  Option<String>,
}

/// op: CHANNEL_LEAVE (12)
#[derive(Deserialize, Debug)]
pub struct ChannelLeavePayload {
    pub channel_id: String,
}

/// op: CHANNEL_UPDATE (13)
#[derive(Deserialize, Debug)]
pub struct ChannelUpdatePayload {
    pub channel_id:   String,
    pub channel_name: String,
}

/// op: CHANNEL_DELETE (14)
#[derive(Deserialize, Debug)]
pub struct ChannelDeletePayload {
    pub channel_id: String,
}

/// op: MESSAGE_CREATE (20)
#[derive(Deserialize, Debug)]
pub struct MessageCreatePayload {
    pub channel_id: String,
    pub content:    String,
}

// ----------------------------------------------------------------------------
// [S→C] 서버 응답 payload 타입들
// ----------------------------------------------------------------------------

/// op: HELLO (0)
#[derive(Serialize, Debug)]
pub struct HelloPayload {
    pub heartbeat_interval: u64,
}

/// op: READY (4)
#[derive(Serialize, Debug)]
pub struct ReadyPayload {
    pub session_id: String,
    pub user_id:    String,
}

/// op: ACK (200)
#[derive(Serialize, Debug)]
pub struct AckPayload {
    pub op:   u8,
    pub data: serde_json::Value,
}

/// op: ACK > CHANNEL_JOIN 성공 시 data
#[derive(Serialize, Debug)]
pub struct ChannelJoinAckData {
    pub channel_id:     String,
    pub sdp_answer:     Option<String>,
    pub active_members: Vec<MemberInfo>,
}

/// op: CHANNEL_EVENT (100)
#[derive(Serialize, Debug)]
pub struct ChannelEventPayload {
    pub event:      String,
    pub channel_id: String,
    pub member:     MemberInfo,
}

/// op: MESSAGE_EVENT (101)
#[derive(Serialize, Debug)]
pub struct MessageEventPayload {
    pub message_id: String,
    pub channel_id: String,
    pub author_id:  String,
    pub content:    String,
    pub timestamp:  u64,
}

/// op: ERROR (201)
#[derive(Serialize, Debug)]
pub struct ErrorPayload {
    pub code:   u16,
    pub reason: String,
}

/// op: ACK > CHANNEL_LIST
#[derive(Serialize, Debug)]
pub struct ChannelSummary {
    pub channel_id:   String,
    pub freq:         String,
    pub name:         String,
    pub mode:         String,
    pub member_count: usize,
    pub capacity:     usize,
    pub created_at:   u64,
}

/// op: ACK > CHANNEL_INFO
#[derive(Serialize, Debug)]
pub struct ChannelInfoData {
    pub channel_id:   String,
    pub freq:         String,
    pub name:         String,
    pub mode:         String,
    pub member_count: usize,
    pub capacity:     usize,
    pub created_at:   u64,
    pub peers:        Vec<MemberInfo>,
}

// ----------------------------------------------------------------------------
// [공통] 멤버 정보
// ----------------------------------------------------------------------------
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MemberInfo {
    pub user_id: String,
    pub ssrc:    u32,
}

// ----------------------------------------------------------------------------
// [헬퍼] 서버 응답 패킷 생성 함수
// ----------------------------------------------------------------------------

impl GatewayPacket {
    pub fn new(op: u8, payload: impl Serialize) -> Self {
        Self {
            op,
            d: Some(serde_json::to_value(payload).unwrap_or(serde_json::Value::Null)),
        }
    }

    pub fn no_data(op: u8) -> Self {
        Self { op, d: None }
    }
}

// ----------------------------------------------------------------------------
// [Floor Control] MBCP TS 24.380 payload 타입
// ----------------------------------------------------------------------------

/// Floor Indicator — 발언의 성격 (직렬화용)
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FloorIndicatorDto {
    Normal,
    Broadcast,
    ImminentPeril,
    Emergency,
}

/// op: FLOOR_REQUEST (30) — C→S, PTT 누름
#[derive(Deserialize, Debug)]
pub struct FloorRequestPayload {
    pub channel_id: String,
    pub priority:   Option<u8>,
    pub indicator:  Option<FloorIndicatorDto>,
}

/// op: FLOOR_RELEASE (31) — C→S, PTT 놓음
#[derive(Deserialize, Debug)]
pub struct FloorReleasePayload {
    pub channel_id: String,
}

/// op: FLOOR_PING (32) — C→S, holder 생존 신호
#[derive(Deserialize, Debug)]
pub struct FloorPingPayload {
    pub channel_id: String,
}

/// op: FLOOR_GRANTED (110) — S→C, 발언권 허가
#[derive(Serialize, Debug)]
pub struct FloorGrantedPayload {
    pub channel_id: String,
    pub user_id:    String,
    pub duration:   u64,   // 최대 발언 가능 시간 ms
}

/// op: FLOOR_DENY (111) — S→C, 발언권 거부
#[derive(Serialize, Debug)]
pub struct FloorDenyPayload {
    pub channel_id: String,
    pub reason:     String,
}

/// op: FLOOR_TAKEN (112) — S→C 브로드캐스트, 누군가 발언 중
#[derive(Serialize, Debug)]
pub struct FloorTakenPayload {
    pub channel_id: String,
    pub user_id:    String,
    pub indicator:  FloorIndicatorDto,
}

/// op: FLOOR_IDLE (113) — S→C 브로드캐스트, 발언권 없음
#[derive(Serialize, Debug)]
pub struct FloorIdlePayload {
    pub channel_id: String,
}

/// op: FLOOR_REVOKE (114) — S→C, 발언권 강제 회수
#[derive(Serialize, Debug)]
pub struct FloorRevokePayload {
    pub channel_id: String,
    pub cause:      String,  // "preempted" | "timeout" | "max_duration" | "disconnect"
}

/// op: FLOOR_QUEUE_POS_INFO (115) — S→C, 대기열 진입 확인
#[derive(Serialize, Debug)]
pub struct FloorQueuePosInfoPayload {
    pub channel_id:     String,
    pub queue_position: usize,
    pub queue_size:     usize,
}

/// op: FLOOR_PONG (116) — S→C, 서버가 holder의 Ping에 응답
#[derive(Serialize, Debug)]
pub struct FloorPongPayload {
    pub channel_id: String,
}
