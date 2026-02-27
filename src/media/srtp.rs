// author: kodeholic (powered by Claude)
// SRTP 컨텍스트 모듈
//
// [Phase 2.1] 키 설치 구조만 구현, 실제 암복호화는 패스스루
// [Phase 2.2] webrtc-srtp Context 기반 실제 암복호화 구현 완료
//
// 사용 프로파일: AES_CM_128_HMAC_SHA1_80 (WebRTC 표준, RFC 5764)
//   master_key  = 16 bytes
//   master_salt = 14 bytes

use webrtc_srtp::context::Context;
use webrtc_srtp::protection_profile::ProtectionProfile;
use tracing::debug;

use crate::core::Endpoint;

// ============================================================================
// [SrtpContext]
// Endpoint당 inbound / outbound 각각 1개
// Context::new() 는 &mut self 메서드(decrypt_rtp/encrypt_rtp)가 필요하므로
// Option<Context>로 보관하고 is_ready()로 초기화 여부 확인
// ============================================================================

pub struct SrtpContext {
    inner: Option<Context>,
}

impl SrtpContext {
    pub fn new() -> Self {
        Self { inner: None }
    }

    /// DTLS 핸드셰이크 완료 후 키 설치
    /// profile: AES_CM_128_HMAC_SHA1_80 (key 16B + salt 14B)
    pub fn install_key(&mut self, key: &[u8], salt: &[u8]) {
        match Context::new(
            key,
            salt,
            ProtectionProfile::Aes128CmHmacSha1_80,
            None, // srtp replay protection: no_replay_protection (기본값)
            None, // srtcp replay protection: no_replay_protection (기본값)
        ) {
            Ok(ctx) => {
                self.inner = Some(ctx);
                debug!("[srtp] context ready key_len={} salt_len={}", key.len(), salt.len());
            }
            Err(e) => {
                // 키 길이/salt 길이 불일치 시 발생 — 절대 여기 오면 안 됨
                tracing::error!("[srtp] Context::new failed: {:?}", e);
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        self.inner.is_some()
    }

    /// 수신 SRTP 패킷 복호화 → plaintext RTP bytes
    /// 키 미설치 시: KeyNotInstalled 에러 반환 (패스스루 없음)
    pub fn decrypt_rtcp(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        match &mut self.inner {
            None => Err(SrtpError::KeyNotInstalled),
            Some(ctx) => ctx
                .decrypt_rtcp(packet)
                .map(|b: bytes::Bytes| b.to_vec())
                .map_err(|e: webrtc_srtp::Error| SrtpError::DecryptFailed(e.to_string())),
        }
    }

    pub fn decrypt(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        match &mut self.inner {
            None => Err(SrtpError::KeyNotInstalled),
            Some(ctx) => ctx
                .decrypt_rtp(packet)
                .map(|b: bytes::Bytes| b.to_vec())
                .map_err(|e: webrtc_srtp::Error| SrtpError::DecryptFailed(e.to_string())),
        }
    }

    /// plaintext RTP 패킷 암호화 → SRTP bytes
    /// 키 미설치 시: KeyNotInstalled 에러 반환 (패스스루 없음)
    pub fn encrypt(&mut self, packet: &[u8]) -> Result<Vec<u8>, SrtpError> {
        match &mut self.inner {
            None => Err(SrtpError::KeyNotInstalled),
            Some(ctx) => ctx
                .encrypt_rtp(packet)
                .map(|b: bytes::Bytes| b.to_vec())
                .map_err(|e: webrtc_srtp::Error| SrtpError::EncryptFailed(e.to_string())),
        }
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
// key material 레이아웃 (RFC 5764 §4.2, AES_CM_128_HMAC_SHA1_80):
//   inbound  키 = client_write_key/salt (브라우저→서버 방향)
//   outbound 키 = server_write_key/salt (서버→브라우저 방향)
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
        if !inbound.is_ready() {
            return Err(SrtpError::DecryptFailed("inbound context init failed".into()));
        }
    }
    {
        let mut outbound = endpoint.outbound_srtp.lock().unwrap();
        outbound.install_key(server_key, server_salt);
        if !outbound.is_ready() {
            return Err(SrtpError::EncryptFailed("outbound context init failed".into()));
        }
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
            SrtpError::InvalidPacket(m)  => write!(f, "Invalid SRTP packet: {}", m),
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

    // 최소 유효 RTP 헤더 (12 bytes)
    // V=2, P=0, X=0, CC=0, M=0, PT=120, seq=1, ts=0, ssrc=0x0001E240
    const SAMPLE_RTP: [u8; 12] = [
        0x80, 0x78, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x01, 0xE2, 0x40,
    ];

    #[test]
    fn new_context_is_not_ready() {
        let ctx = SrtpContext::new();
        assert!(!ctx.is_ready());
    }

    #[test]
    fn key_install_marks_ready() {
        let mut ctx = SrtpContext::new();
        ctx.install_key(&[0u8; 16], &[0u8; 14]);
        assert!(ctx.is_ready());
    }

    #[test]
    fn decrypt_before_key_returns_error() {
        let mut ctx = SrtpContext::new();
        assert!(matches!(ctx.decrypt(&SAMPLE_RTP), Err(SrtpError::KeyNotInstalled)));
    }

    #[test]
    fn encrypt_before_key_returns_error() {
        let mut ctx = SrtpContext::new();
        assert!(matches!(ctx.encrypt(&SAMPLE_RTP), Err(SrtpError::KeyNotInstalled)));
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        // 동일 키/salt로 encrypt 후 decrypt하면 원본 복원되어야 함
        let key  = [0x01u8; 16];
        let salt = [0x02u8; 14];

        // RTP 페이로드 포함 패킷 (헤더 12B + 페이로드 4B)
        let plaintext: Vec<u8> = vec![
            0x80, 0x78, 0x00, 0x01,   // RTP header
            0x00, 0x00, 0x00, 0x00,
            0x00, 0x01, 0xE2, 0x40,
            0xDE, 0xAD, 0xBE, 0xEF,   // payload
        ];

        let mut enc_ctx = SrtpContext::new();
        enc_ctx.install_key(&key, &salt);

        let mut dec_ctx = SrtpContext::new();
        dec_ctx.install_key(&key, &salt);

        let encrypted = enc_ctx.encrypt(&plaintext).expect("encrypt failed");
        let decrypted = dec_ctx.decrypt(&encrypted).expect("decrypt failed");

        assert_eq!(decrypted, plaintext);
    }
}
