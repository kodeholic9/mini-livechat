// author: kodeholic (powered by Claude)

use crate::error::LiveError;

/// 1xxx: 연결/인증
pub const NOT_AUTHENTICATED:  u16 = 1000;
pub const INVALID_TOKEN:      u16 = 1001;
pub const INVALID_OPCODE:     u16 = 1003;
pub const INVALID_PAYLOAD:    u16 = 1004;

/// 2xxx: 채널
pub const CHANNEL_NOT_FOUND:  u16 = 2000;
pub const CHANNEL_FULL:       u16 = 2001;
pub const CHANNEL_ACCESS_DENIED: u16 = 2002;
pub const ALREADY_IN_CHANNEL: u16 = 2003;
pub const NOT_IN_CHANNEL:     u16 = 2004;

/// 3xxx: 메시지
pub const EMPTY_MESSAGE:      u16 = 3000;
pub const MESSAGE_TOO_LONG:   u16 = 3001;
pub const MESSAGE_NOT_IN_CHANNEL: u16 = 3002;

/// 9xxx: 서버 내부
pub const INTERNAL_ERROR:     u16 = 9000;

/// LiveError → 에러 코드 변환
/// 에러 응답 패킷 생성 시 사용
pub fn to_error_code(err: &LiveError) -> u16 {
    match err {
        LiveError::NotAuthenticated        => NOT_AUTHENTICATED,
        LiveError::InvalidToken            => INVALID_TOKEN,
        LiveError::InvalidOpcode(_)        => INVALID_OPCODE,
        LiveError::InvalidPayload(_)       => INVALID_PAYLOAD,

        LiveError::ChannelNotFound(_)      => CHANNEL_NOT_FOUND,
        LiveError::ChannelFull(_)          => CHANNEL_FULL,
        LiveError::ChannelAccessDenied(_)  => CHANNEL_ACCESS_DENIED,
        LiveError::AlreadyInChannel(_)     => ALREADY_IN_CHANNEL,
        LiveError::NotInChannel(_)         => NOT_IN_CHANNEL,

        LiveError::EmptyMessage            => EMPTY_MESSAGE,
        LiveError::MessageTooLong(_)       => MESSAGE_TOO_LONG,
        LiveError::MessageNotInChannel(_)  => MESSAGE_NOT_IN_CHANNEL,

        LiveError::InternalError(_)
        | LiveError::IoError(_)            => INTERNAL_ERROR,
    }
}
