use serde::{Deserialize, Serialize};

/// 클라이언트 -> 서버 (요청)
#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClientMessage {
    /// 방 입장 요청
    Join { 
        ssrc: u32, 
        channel_id: String,
        // (향후 추가) 암호화 키 등 SDP 정보
    },
    /// 방 퇴장 요청
    Leave { 
        ssrc: u32 
    },
}

/// 서버 -> 클라이언트 (응답 및 브로드캐스트)
#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum ServerMessage {
    /// 입장 성공 응답
    Joined { 
        ssrc: u32, 
        channel_id: String 
    },
    /// 에러 발생 (방 꽉 참 등)
    Error { 
        code: u16, 
        reason: String 
    },
}