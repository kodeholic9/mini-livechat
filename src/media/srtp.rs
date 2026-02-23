// author: kodeholic (powered by Claude)
// RTP 미디어 패킷 처리 모듈
//
// [현재 상태] Phase 1: 평문 패스스루
//   - 암복호화 없이 수신 패킷을 그대로 반환합니다.
//   - 내부망 / 개발 환경 용도로 사용합니다.
//
// [Phase 2 예정] SRTP 암복호화
//   - 앱 클라이언트: WS 시그널링으로 pre-shared key 교환 후 AES-128-CTR
//   - 브라우저 클라이언트: DTLS-SRTP (별도 구현 필요)
//
// 흐름:
//   수신 패킷 → decrypt() → plaintext RTP
//   plaintext RTP → encrypt() → 송신 패킷

/// RTP/SRTP 세션 컨텍스트
/// 피어마다 inbound / outbound 각각 보유합니다.
///
/// Phase 2에서 아래 필드가 추가됩니다:
///   master_key:  [u8; 16]
///   master_salt: [u8; 14]
pub struct SrtpContext;

impl SrtpContext {
    pub fn new() -> Self {
        Self
    }

    /// 수신 패킷 복호화 → plaintext RTP 반환
    /// Phase 1: 패스스루 (그대로 반환)
    pub fn decrypt<'a>(&self, packet: &'a [u8]) -> Result<&'a [u8], SrtpError> {
        Ok(packet)
    }

    /// plaintext RTP 암호화 → 송신 패킷 반환
    /// Phase 1: 패스스루 (그대로 반환)
    pub fn encrypt<'a>(&self, packet: &'a [u8]) -> Result<&'a [u8], SrtpError> {
        Ok(packet)
    }
}

impl Default for SrtpContext {
    fn default() -> Self {
        Self::new()
    }
}

/// RTP/SRTP 처리 에러
#[derive(Debug)]
pub enum SrtpError {
    DecryptFailed(String),
    EncryptFailed(String),
    InvalidPacket(String),
}

impl std::fmt::Display for SrtpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SrtpError::DecryptFailed(msg)  => write!(f, "RTP decrypt failed: {}", msg),
            SrtpError::EncryptFailed(msg)  => write!(f, "RTP encrypt failed: {}", msg),
            SrtpError::InvalidPacket(msg)  => write!(f, "Invalid RTP packet: {}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 최소 RTP 헤더 12바이트 (V=2, PT=120, Seq=1, TS=0, SSRC=0x0001E240)
    const SAMPLE_RTP: [u8; 12] = [0x80, 0x78, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xE2, 0x40];

    #[test]
    fn passthrough_decrypt_returns_original() {
        let ctx    = SrtpContext::new();
        let result = ctx.decrypt(&SAMPLE_RTP).unwrap();
        assert_eq!(result, &SAMPLE_RTP);
    }

    #[test]
    fn passthrough_encrypt_returns_original() {
        let ctx    = SrtpContext::new();
        let result = ctx.encrypt(&SAMPLE_RTP).unwrap();
        assert_eq!(result, &SAMPLE_RTP);
    }

    #[test]
    fn empty_packet_passthrough() {
        let ctx = SrtpContext::new();
        assert_eq!(ctx.decrypt(&[]).unwrap(), &[] as &[u8]);
        assert_eq!(ctx.encrypt(&[]).unwrap(), &[] as &[u8]);
    }
}
