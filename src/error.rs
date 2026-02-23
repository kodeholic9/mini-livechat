// author: kodeholic (powered by Claude)

use std::fmt;

/// 시스템 전체 에러 타입
/// protocol::error_code 에서 u16 에러 코드로 변환됩니다.
#[derive(Debug)]
pub enum LiveError {
    // 1xxx: 연결/인증
    NotAuthenticated,
    InvalidToken,
    InvalidOpcode(u8),
    InvalidPayload(String),

    // 2xxx: 채널
    ChannelNotFound(String),
    ChannelFull(String),
    ChannelAccessDenied(String),
    AlreadyInChannel(String),
    NotInChannel(String),

    // 3xxx: 메시지
    EmptyMessage,
    MessageTooLong(usize),
    MessageNotInChannel(String),

    // 9xxx: 서버 내부
    InternalError(String),
    IoError(std::io::Error),
}

impl fmt::Display for LiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LiveError::NotAuthenticated         => write!(f, "Authentication required"),
            LiveError::InvalidToken             => write!(f, "Invalid or expired token"),
            LiveError::InvalidOpcode(op)        => write!(f, "Unknown opcode: {}", op),
            LiveError::InvalidPayload(msg)      => write!(f, "Invalid payload: {}", msg),

            LiveError::ChannelNotFound(id)      => write!(f, "Channel not found: {}", id),
            LiveError::ChannelFull(id)          => write!(f, "Channel is full: {}", id),
            LiveError::ChannelAccessDenied(id)  => write!(f, "Access denied to channel: {}", id),
            LiveError::AlreadyInChannel(id)     => write!(f, "Already in channel: {}", id),
            LiveError::NotInChannel(id)         => write!(f, "Not in channel: {}", id),

            LiveError::EmptyMessage             => write!(f, "Message content is empty"),
            LiveError::MessageTooLong(len)      => write!(f, "Message too long: {} chars", len),
            LiveError::MessageNotInChannel(id)  => write!(f, "Must join channel before messaging: {}", id),

            LiveError::InternalError(msg)       => write!(f, "Internal server error: {}", msg),
            LiveError::IoError(err)             => write!(f, "I/O error: {}", err),
        }
    }
}

impl std::error::Error for LiveError {}

pub type LiveResult<T> = Result<T, LiveError>;
