// author: kodeholic (powered by Claude)
// DTLS Passive 핸드셰이크 모듈 (Phase 2)
//
// 역할:
//   1. 서버 시작 시 자체서명 인증서 생성 (1회)
//   2. 클라이언트 DTLS ClientHello 수신 → passive 핸드셰이크
//   3. 핸드셰이크 완료 → RFC 5705 키 도출 → SRTP 컨텍스트 초기화
//
// RFC 5764 §4.2 키 머티리얼 레이아웃 (AES_CM_128_HMAC_SHA1_80 기준, 60바이트):
//   [0..16]   client_write_key  (16)  — inbound (브라우저→서버)
//   [16..32]  server_write_key  (16)  — outbound (서버→브라우저)
//   [32..46]  client_write_salt (14)
//   [46..60]  server_write_salt (14)

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use webrtc_util::KeyingMaterialExporter;
use sha2::{Digest, Sha256};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::core::Endpoint;
use crate::media::srtp::init_srtp_contexts;

// RFC 5764 §4.2 — SRTP-DTLS 키 도출 상수
const SRTP_MASTER_KEY_LABEL: &str  = "EXTRACTOR-dtls_srtp";
const SRTP_MASTER_KEY_LEN:   usize = 16;
const SRTP_MASTER_SALT_LEN:  usize = 14;
// 레이아웃: client_key | server_key | client_salt | server_salt
const SRTP_KEY_MATERIAL_LEN: usize = (SRTP_MASTER_KEY_LEN + SRTP_MASTER_SALT_LEN) * 2;

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

    /// tx가 닫힌 세션 = 핸드셰이크 태스크 종료 또는 타임아웃
    /// 제거 후 제거된 주소 목록 반환
    pub async fn remove_stale(&self) -> Vec<SocketAddr> {
        let stale: Vec<SocketAddr> = self.sessions.read().await
            .iter()
            .filter(|(_, tx)| tx.is_closed())
            .map(|(addr, _)| *addr)
            .collect();

        if !stale.is_empty() {
            let mut sessions = self.sessions.write().await;
            for addr in &stale {
                sessions.remove(addr);
                debug!("[dtls-map] stale session removed addr={}", addr);
            }
        }

        stale
    }
}

// ============================================================================
// [ServerCert]
// ============================================================================

pub struct ServerCert {
    pub dtls_cert:   dtls::crypto::Certificate,
    pub fingerprint: String,
}

impl ServerCert {
    pub fn generate() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let dtls_cert = dtls::crypto::Certificate::generate_self_signed(
            vec!["mini-livechat".to_string()]
        )?;

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
        let timeout = tokio::time::Duration::from_millis(crate::config::DTLS_HANDSHAKE_TIMEOUT_MS);
        let result  = tokio::time::timeout(timeout, do_handshake(Arc::new(adapter), &endpoint, &cert)).await;
        session_map2.remove(&peer_addr).await;

        match result {
            Ok(Ok(()))  => info!("[dtls] session complete user={} addr={}", endpoint.user_id, peer_addr),
            Ok(Err(e))  => warn!("[dtls] session failed user={} addr={}: {}", endpoint.user_id, peer_addr, e),
            Err(_)      => warn!("[dtls] handshake timeout user={} addr={}", endpoint.user_id, peer_addr),
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
    // RFC 5705 키 도출
    //
    // DTLSConn.state 는 pub(crate) 라 직접 접근 불가.
    // connection_state() 로 State 복사본을 꺼낸 뒤
    // KeyingMaterialExporter 트레이트의 export_keying_material() 호출.
    //
    // context 파라미터는 반드시 &[] (비어있지 않으면 ContextUnsupported 에러)
    // ─────────────────────────────────────────────────────────────
    let state = dtls_conn.connection_state().await;
    let material: Vec<u8> = state
        .export_keying_material(SRTP_MASTER_KEY_LABEL, &[], SRTP_KEY_MATERIAL_LEN)
        .await
        .map_err(|e| format!("export_keying_material failed: {e:?}"))?;

    // RFC 5764 §4.2 레이아웃 슬라이싱
    let client_key  = &material[0..SRTP_MASTER_KEY_LEN];
    let server_key  = &material[SRTP_MASTER_KEY_LEN..SRTP_MASTER_KEY_LEN * 2];
    let client_salt = &material[SRTP_MASTER_KEY_LEN * 2..SRTP_MASTER_KEY_LEN * 2 + SRTP_MASTER_SALT_LEN];
    let server_salt = &material[SRTP_MASTER_KEY_LEN * 2 + SRTP_MASTER_SALT_LEN..];

    init_srtp_contexts(endpoint, client_key, client_salt, server_key, server_salt)?;
    info!("[dtls] SRTP keys installed user={}", endpoint.user_id);

    // dtls_conn 유지: application data 읽기 루프
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
