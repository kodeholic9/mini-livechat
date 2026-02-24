// author: kodeholic (powered by Claude)
// SRTP 컨텍스트 모듈
//
// [Phase 2.1] 키 설치 구조만 구현, 실제 암복호화는 패스스루
// [Phase 2.2] webrtc-srtp Session 기반 실제 암복호화 구현 예정

use tracing::debug;
use crate::core::Endpoint;

// ============================================================================
// [SrtpContext]
// Endpoint당 inbound / outbound 각각 1개
// ============================================================================

pub struct SrtpContext {
    key_material: Option<KeyMaterial>,
}

#[derive(Clone)]
pub struct KeyMaterial {
    pub master_key:  Vec<u8>,
    pub master_salt: Vec<u8>,
}

impl SrtpContext {
    pub fn new() -> Self {
        Self { key_material: None }
    }

    /// DTLS 핸드셰이크 완료 후 키 설치
    pub fn install_key(&mut self, key: &[u8], salt: &[u8]) {
        self.key_material = Some(KeyMaterial {
            master_key:  key.to_vec(),
            master_salt: salt.to_vec(),
        });
        debug!("[srtp] key installed key_len={} salt_len={}", key.len(), salt.len());
    }

    pub fn is_ready(&self) -> bool {
        self.key_material.is_some()
    }

    /// 수신 패킷 복호화
    /// Phase 2.1: 패스스루 (키 설치 여부와 무관)
    /// Phase 2.2: webrtc-srtp 실제 복호화
    pub fn decrypt<'a>(&self, packet: &'a [u8]) -> Result<&'a [u8], SrtpError> {
        Ok(packet)
    }

    /// plaintext RTP 암호화
    pub fn encrypt<'a>(&self, packet: &'a [u8]) -> Result<&'a [u8], SrtpError> {
        Ok(packet)
    }
}

impl Default for SrtpContext {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// [init_srtp_contexts]
// dtls.rs에서 DTLS 핸드셰이크 완료 시 호출
//
// key material 레이아웃 (RFC 5764, AES_CM_128_HMAC_SHA1_80):
//   inbound  키 = client_write_key/salt (브라우저→서버)
//   outbound 키 = server_write_key/salt (서버→브라우저)
// ============================================================================

pub fn init_srtp_contexts(
    endpoint:    &Endpoint,
    client_key:  &[u8],
    client_salt: &[u8],
    server_key:  &[u8],
    server_salt: &[u8],
) -> Result<(), SrtpError> {
    {
        let mut inbound = endpoint.inbound_srtp.lock().unwrap();
        inbound.install_key(client_key, client_salt);
    }
    {
        let mut outbound = endpoint.outbound_srtp.lock().unwrap();
        outbound.install_key(server_key, server_salt);
    }
    Ok(())
}

// ============================================================================
// [SrtpError]
// ============================================================================

#[derive(Debug)]
pub enum SrtpError {
    DecryptFailed(String),
    EncryptFailed(String),
    InvalidPacket(String),
    KeyNotInstalled,
}

impl std::fmt::Display for SrtpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SrtpError::DecryptFailed(m)  => write!(f, "SRTP decrypt failed: {}", m),
            SrtpError::EncryptFailed(m)  => write!(f, "SRTP encrypt failed: {}", m),
            SrtpError::InvalidPacket(m)  => write!(f, "Invalid packet: {}", m),
            SrtpError::KeyNotInstalled   => write!(f, "SRTP key not installed"),
        }
    }
}

impl std::error::Error for SrtpError {}

// ============================================================================
// [테스트]
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_RTP: [u8; 12] = [
        0x80, 0x78, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0xE2, 0x40,
    ];

    #[test]
    fn passthrough_before_key() {
        let ctx = SrtpContext::new();
        assert!(!ctx.is_ready());
        assert_eq!(ctx.decrypt(&SAMPLE_RTP).unwrap(), &SAMPLE_RTP);
        assert_eq!(ctx.encrypt(&SAMPLE_RTP).unwrap(), &SAMPLE_RTP);
    }

    #[test]
    fn key_install_marks_ready() {
        let mut ctx = SrtpContext::new();
        ctx.install_key(&[0u8; 16], &[0u8; 14]);
        assert!(ctx.is_ready());
    }

    #[test]
    fn passthrough_after_key() {
        let mut ctx = SrtpContext::new();
        ctx.install_key(&[0u8; 16], &[0u8; 14]);
        assert_eq!(ctx.decrypt(&SAMPLE_RTP).unwrap(), &SAMPLE_RTP);
    }
}
