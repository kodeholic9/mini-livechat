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

impl LiveError {
    pub fn code(&self) -> u16 {
        match self {
            // 1xxx: 연결/인증
            LiveError::NotAuthenticated        => 1000,
            LiveError::InvalidToken            => 1001,
            LiveError::InvalidOpcode(_)        => 1003,
            LiveError::InvalidPayload(_)       => 1004,

            // 2xxx: 채널
            LiveError::ChannelNotFound(_)      => 2000,
            LiveError::ChannelFull(_)          => 2001,
            LiveError::ChannelAccessDenied(_)  => 2002,
            LiveError::AlreadyInChannel(_)     => 2003,
            LiveError::NotInChannel(_)         => 2004,

            // 3xxx: 메시지
            LiveError::EmptyMessage            => 3000,
            LiveError::MessageTooLong(_)       => 3001,
            LiveError::MessageNotInChannel(_)  => 3002,

            // 9xxx: 서버 내부
            LiveError::InternalError(_)
            | LiveError::IoError(_)            => 9000,
        }
    }
}

pub type LiveResult<T> = Result<T, LiveError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_1xxx_auth() {
        assert_eq!(LiveError::NotAuthenticated.code(), 1000);
        assert_eq!(LiveError::InvalidToken.code(), 1001);
        assert_eq!(LiveError::InvalidOpcode(99).code(), 1003);
        assert_eq!(LiveError::InvalidPayload("x".into()).code(), 1004);
    }

    #[test]
    fn error_codes_2xxx_channel() {
        assert_eq!(LiveError::ChannelNotFound("c".into()).code(), 2000);
        assert_eq!(LiveError::ChannelFull("c".into()).code(), 2001);
        assert_eq!(LiveError::ChannelAccessDenied("c".into()).code(), 2002);
        assert_eq!(LiveError::AlreadyInChannel("c".into()).code(), 2003);
        assert_eq!(LiveError::NotInChannel("c".into()).code(), 2004);
    }

    #[test]
    fn error_codes_3xxx_message() {
        assert_eq!(LiveError::EmptyMessage.code(), 3000);
        assert_eq!(LiveError::MessageTooLong(9999).code(), 3001);
        assert_eq!(LiveError::MessageNotInChannel("c".into()).code(), 3002);
    }

    #[test]
    fn error_codes_9xxx_internal() {
        assert_eq!(LiveError::InternalError("e".into()).code(), 9000);
    }

    #[test]
    fn display_contains_context() {
        let e = LiveError::ChannelFull("CH_001".into());
        let msg = format!("{}", e);
        assert!(msg.contains("CH_001"));
    }

    #[test]
    fn error_code_ranges_no_overlap() {
        // 모든 코드가 정의된 범위 내에 있는지 확인
        let codes = vec![
            LiveError::NotAuthenticated.code(),
            LiveError::InvalidToken.code(),
            LiveError::InvalidOpcode(0).code(),
            LiveError::InvalidPayload(String::new()).code(),
            LiveError::ChannelNotFound(String::new()).code(),
            LiveError::ChannelFull(String::new()).code(),
            LiveError::ChannelAccessDenied(String::new()).code(),
            LiveError::AlreadyInChannel(String::new()).code(),
            LiveError::NotInChannel(String::new()).code(),
            LiveError::EmptyMessage.code(),
            LiveError::MessageTooLong(0).code(),
            LiveError::MessageNotInChannel(String::new()).code(),
            LiveError::InternalError(String::new()).code(),
        ];
        for &c in &codes {
            let range_ok = (1000..2000).contains(&c)
                || (2000..3000).contains(&c)
                || (3000..4000).contains(&c)
                || (9000..10000).contains(&c);
            assert!(range_ok, "code {} out of defined ranges", c);
        }
    }
}
