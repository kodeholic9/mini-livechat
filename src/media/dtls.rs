// author: kodeholic (powered by Claude)
// DTLS Passive 핸드셰이크 모듈 (Phase 2)
//
// 역할:
//   1. 서버 시작 시 자체서명 인증서 생성 (1회)
//   2. 클라이언트 DTLS ClientHello 수신 → passive 핸드셰이크
//   3. 핸드셰이크 완료 → RFC 5705 키 도출 → SRTP 컨텍스트 초기화
//
// ─────────────────────────────────────────────────────────────────
// [TODO: DTLS-SRTP 키 도출 미완성]
//
//   dtls::conn::DTLSConn 에 export_keying_material() 메서드가
//   공개 API로 노출되지 않음 (0.17.1 기준 확인 필요).
//
//   확인 필요:
//     %USERPROFILE%\.cargo\registry\src\...\dtls-0.17.1\src\conn\mod.rs
//     %USERPROFILE%\.cargo\registry\src\...\rcgen-0.13.2\src\certificate.rs
//
//   현재 상태: 핸드셰이크 완료까지 동작, 키 설치만 TODO
//   CHANGELOG.md 에 상세 기재됨
// ─────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use rustls_pki_types::CertificateDer;
use sha2::{Digest, Sha256};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::core::Endpoint;

// SRTP 키 도출 라벨 (RFC 5764 §4.2) — TODO 완성 시 사용
const _SRTP_MASTER_KEY_LABEL:   &str  = "EXTRACTOR-dtls_srtp";
const _SRTP_MASTER_KEY_LEN:     usize = 16;
const _SRTP_MASTER_SALT_LEN:    usize = 14;
const _SRTP_KEY_MATERIAL_LEN:   usize = (_SRTP_MASTER_KEY_LEN + _SRTP_MASTER_SALT_LEN) * 2;

// ============================================================================
// [DtlsPacketTx]
// ============================================================================

pub type DtlsPacketTx = mpsc::Sender<Vec<u8>>;

// ============================================================================
// [DtlsSessionMap]
// ============================================================================

pub struct DtlsSessionMap {
    sessions: RwLock<HashMap<SocketAddr, DtlsPacketTx>>,
}

impl DtlsSessionMap {
    pub fn new() -> Self {
        Self { sessions: RwLock::new(HashMap::new()) }
    }

    pub async fn insert(&self, addr: SocketAddr, tx: DtlsPacketTx) {
        self.sessions.write().await.insert(addr, tx);
        debug!("[dtls-map] session registered addr={}", addr);
    }

    pub async fn remove(&self, addr: &SocketAddr) {
        self.sessions.write().await.remove(addr);
        debug!("[dtls-map] session removed addr={}", addr);
    }

    pub async fn inject(&self, addr: &SocketAddr, packet: Vec<u8>) -> bool {
        if let Some(tx) = self.sessions.read().await.get(addr) {
            tx.send(packet).await.is_ok()
        } else {
            false
        }
    }
}

// ============================================================================
// [ServerCert]
//
// 인증서 생성 방식:
//   dtls::crypto::Certificate 는 내부적으로 rcgen을 사용하므로,
//   rcgen 버전 충돌을 피하기 위해 dtls가 노출하는 generate() 헬퍼를 사용.
//   dtls 0.17.1에 generate() 헬퍼가 없다면 rcgen을 dtls 내부 버전과
//   동일 버전으로 맞춰 직접 생성.
// ============================================================================

pub struct ServerCert {
    pub dtls_cert:   dtls::crypto::Certificate,
    pub fingerprint: String,
}

impl ServerCert {
    pub fn generate() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // dtls::crypto::Certificate::generate() 헬퍼 시도
        // 내부적으로 rcgen을 사용해 자체서명 인증서를 생성
        let dtls_cert = dtls::crypto::Certificate::generate_self_signed(
            vec!["mini-livechat".to_string()]
        )?;

        // DER 바이트 추출 (fingerprint 계산용)
        let cert_der: Vec<u8> = dtls_cert.certificate
            .first()
            .map(|c| c.to_vec())
            .unwrap_or_default();

        let fingerprint = sha256_fingerprint(&cert_der);
        info!("[dtls] Server cert generated. fingerprint={:.47}...", fingerprint);

        Ok(Self { dtls_cert, fingerprint })
    }
}

// ============================================================================
// [start_dtls_handshake]
// ============================================================================

pub async fn start_dtls_handshake(
    socket:      Arc<UdpSocket>,
    peer_addr:   SocketAddr,
    endpoint:    Arc<Endpoint>,
    cert:        Arc<ServerCert>,
    session_map: Arc<DtlsSessionMap>,
) {
    let (adapter, pkt_tx) = UdpConnAdapter::new(Arc::clone(&socket), peer_addr);
    session_map.insert(peer_addr, pkt_tx).await;

    let session_map2 = Arc::clone(&session_map);
    tokio::spawn(async move {
        let result = do_handshake(Arc::new(adapter), &endpoint, &cert).await;
        session_map2.remove(&peer_addr).await;

        match result {
            Ok(())  => info!("[dtls] session complete user={} addr={}", endpoint.user_id, peer_addr),
            Err(e)  => warn!("[dtls] session failed user={} addr={}: {}", endpoint.user_id, peer_addr, e),
        }
    });
}

async fn do_handshake(
    conn:     Arc<UdpConnAdapter>,
    endpoint: &Endpoint,
    cert:     &ServerCert,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    debug!("[dtls] passive handshake start user={}", endpoint.user_id);

    use dtls::extension::extension_use_srtp::SrtpProtectionProfile;

    let config = dtls::config::Config {
        certificates: vec![cert.dtls_cert.clone()],
        srtp_protection_profiles: vec![
            SrtpProtectionProfile::Srtp_Aes128_Cm_Hmac_Sha1_80,
        ],
        extended_master_secret: dtls::config::ExtendedMasterSecretType::Require,
        insecure_skip_verify: true,
        ..Default::default()
    };

    let dtls_conn = dtls::conn::DTLSConn::new(
        conn,
        config,
        false, // passive (서버)
        None,
    ).await?;

    info!("[dtls] handshake complete user={}", endpoint.user_id);

    // ─────────────────────────────────────────────────────────────
    // TODO: SRTP 키 도출 — export_keying_material() 메서드 미확인
    //
    // dtls/src/conn/mod.rs 에서 pub fn 목록 확인 후 아래 코드로 교체:
    //
    // use crate::media::srtp::init_srtp_contexts;
    // let material = dtls_conn
    //     .export_keying_material(_SRTP_MASTER_KEY_LABEL, &[], _SRTP_KEY_MATERIAL_LEN)
    //     .await?;
    // let cw_key  = &material[0.._SRTP_MASTER_KEY_LEN];
    // let sw_key  = &material[_SRTP_MASTER_KEY_LEN.._SRTP_MASTER_KEY_LEN * 2];
    // let cw_salt = &material[_SRTP_MASTER_KEY_LEN * 2.._SRTP_MASTER_KEY_LEN * 2 + _SRTP_MASTER_SALT_LEN];
    // let sw_salt = &material[_SRTP_MASTER_KEY_LEN * 2 + _SRTP_MASTER_SALT_LEN..];
    // init_srtp_contexts(endpoint, cw_key, cw_salt, sw_key, sw_salt)?;
    // ─────────────────────────────────────────────────────────────
    warn!("[dtls] TODO: SRTP key export not yet implemented user={}", endpoint.user_id);

    // dtls_conn drop 방지: application data 읽기 루프 (현재 미사용)
    let mut buf = vec![0u8; 1500];
    loop {
        match dtls_conn.read(&mut buf, None).await {
            Ok(0) | Err(_) => break,
            Ok(_)          => {}
        }
    }

    Ok(())
}

// ============================================================================
// [UdpConnAdapter]
// ============================================================================

pub struct UdpConnAdapter {
    socket:    Arc<UdpSocket>,
    peer_addr: SocketAddr,
    rx:        Mutex<mpsc::Receiver<Vec<u8>>>,
}

impl UdpConnAdapter {
    pub fn new(socket: Arc<UdpSocket>, peer_addr: SocketAddr) -> (Self, DtlsPacketTx) {
        let (tx, rx) = mpsc::channel(128);
        let adapter = Self { socket, peer_addr, rx: Mutex::new(rx) };
        (adapter, tx)
    }
}

#[async_trait]
impl webrtc_util::Conn for UdpConnAdapter {
    async fn connect(&self, _addr: SocketAddr) -> webrtc_util::Result<()> { Ok(()) }

    async fn recv(&self, buf: &mut [u8]) -> webrtc_util::Result<usize> {
        let mut rx = self.rx.lock().await;
        match rx.recv().await {
            Some(data) => {
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
                Ok(len)
            }
            None => Err(webrtc_util::Error::Other("dtls rx channel closed".to_string())),
        }
    }

    async fn recv_from(&self, buf: &mut [u8]) -> webrtc_util::Result<(usize, SocketAddr)> {
        let n = self.recv(buf).await?;
        Ok((n, self.peer_addr))
    }

    async fn send(&self, buf: &[u8]) -> webrtc_util::Result<usize> {
        self.socket.send_to(buf, self.peer_addr).await
            .map_err(|e| webrtc_util::Error::Other(e.to_string()))
    }

    async fn send_to(&self, buf: &[u8], _target: SocketAddr) -> webrtc_util::Result<usize> {
        self.send(buf).await
    }

    fn local_addr(&self) -> webrtc_util::Result<SocketAddr> {
        self.socket.local_addr()
            .map_err(|e| webrtc_util::Error::Other(e.to_string()))
    }

    fn remote_addr(&self) -> Option<SocketAddr> { Some(self.peer_addr) }

    async fn close(&self) -> webrtc_util::Result<()> { Ok(()) }

    fn as_any(&self) -> &(dyn std::any::Any + Send + Sync) { self }
}

// ============================================================================
// [유틸]
// ============================================================================

fn sha256_fingerprint(der: &[u8]) -> String {
    let hash = Sha256::digest(der);
    let hex: Vec<String> = hash.iter().map(|b| format!("{:02X}", b)).collect();
    format!("sha-256 {}", hex.join(":"))
}
