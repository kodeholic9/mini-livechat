// author: kodeholic (powered by Claude)

use serde::{Deserialize, Serialize};

// ----------------------------------------------------------------------------
// [공통] Gateway 패킷 봉투 (Envelope)
// ----------------------------------------------------------------------------

/// 모든 WebSocket 메시지의 최상위 구조체
/// 수신/송신 공통으로 사용하며, payload는 op에 따라 해석합니다.
///
/// 예시:
///   { "op": 11, "d": { "channel_id": "CH_001", "ssrc": 12345 } }
#[derive(Serialize, Deserialize, Debug)]
pub struct GatewayPacket {
    /// opcode (protocol::opcode 참조)
    pub op: u8,
    /// payload. op에 따라 구조가 달라지므로 raw JSON으로 보관
    pub d: Option<serde_json::Value>,
}

// ----------------------------------------------------------------------------
// [C→S] 클라이언트 요청 payload 타입들
// ----------------------------------------------------------------------------

/// op: IDENTIFY (3)
#[derive(Deserialize, Debug)]
pub struct IdentifyPayload {
    pub user_id: String,
    pub token:   String,
}

/// op: CHANNEL_CREATE (10)
#[derive(Deserialize, Debug)]
pub struct ChannelCreatePayload {
    pub channel_id:   String,
    pub channel_name: String,
}

/// op: CHANNEL_JOIN (11)
#[derive(Deserialize, Debug)]
pub struct ChannelJoinPayload {
    pub channel_id: String,
    pub ssrc:       u32,
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

/// op: HELLO (0) — 연결 직후 heartbeat 주기 안내
#[derive(Serialize, Debug)]
pub struct HelloPayload {
    pub heartbeat_interval: u64,
}

/// op: READY (4) — IDENTIFY 성공 응답
#[derive(Serialize, Debug)]
pub struct ReadyPayload {
    pub session_id: String,
    pub user_id:    String,
}

/// op: ACK (200) — 요청 성공 응답
/// data는 op마다 다르므로 raw Value 사용
#[derive(Serialize, Debug)]
pub struct AckPayload {
    pub op:   u8,
    pub data: serde_json::Value,
}

/// op: ACK > CHANNEL_JOIN 성공 시 data 내용
#[derive(Serialize, Debug)]
pub struct ChannelJoinAckData {
    pub channel_id:     String,
    pub active_members: Vec<MemberInfo>,
}

/// op: CHANNEL_EVENT (100) — 채널 멤버 변동 브로드캐스트
#[derive(Serialize, Debug)]
pub struct ChannelEventPayload {
    /// "join" | "leave" | "update"
    pub event:      String,
    pub channel_id: String,
    pub member:     MemberInfo,
}

/// op: MESSAGE_EVENT (101) — 채팅 메시지 브로드캐스트
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
