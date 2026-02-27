// author: kodeholic (powered by Claude)
// UDP 미디어 릴레이 모듈 (Phase 2)
//
// 패킷 수신 흐름:
//   recv_from(src_addr)
//     → 패킷 타입 판별 (STUN / DTLS / SRTP)
//     → STUN : ufrag 파싱 → latch → Binding Response
//     → DTLS : DtlsSessionMap 조회 → 기존 세션에 주입 or 신규 핸드셰이크 시작
//     → SRTP : by_addr O(1) 조회 → 복호화 → 채널 내 다른 피어 재암호화 → 릴레이

use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, info, trace, warn};

use crate::config;
use crate::core::MediaPeerHub;
use crate::media::dtls::{DtlsSessionMap, ServerCert, start_dtls_handshake};

const UDP_RECV_BUF_SIZE: usize = 65535;

// ----------------------------------------------------------------------------
// [패킷 타입 판별]
//
// RFC 5764 §5.1.2 demux 규칙:
//   STUN  : 첫 바이트 0x00 or 0x01
//   DTLS  : 첫 바이트 0x14~0x1F
//   SRTP  : 첫 바이트 0x80~0xFF
// ----------------------------------------------------------------------------

#[derive(Debug)]
enum PacketKind {
    Stun,
    Dtls,
    Srtp,
    Unknown,
}

#[inline]
fn classify(buf: &[u8]) -> PacketKind {
    match buf.first() {
        Some(0x00) | Some(0x01)              => PacketKind::Stun,
        Some(b) if *b >= 0x14 && *b <= 0x1F => PacketKind::Dtls,
        Some(b) if *b >= 0x80               => PacketKind::Srtp,
        _                                    => PacketKind::Unknown,
    }
}

// ----------------------------------------------------------------------------
// [UDP 릴레이 서버]
// ----------------------------------------------------------------------------

pub async fn run_udp_relay(
    hub:         Arc<MediaPeerHub>,
    cert:        Arc<ServerCert>,
    session_map: Arc<DtlsSessionMap>,
) {
    let addr   = format!("0.0.0.0:{}", config::SERVER_UDP_PORT);
    let socket = match UdpSocket::bind(&addr).await {
        Ok(s)  => { info!("[media] UDP relay on {}", addr); Arc::new(s) }
        Err(e) => { tracing::error!("[media] bind failed: {}", e); return; }
    };

    let mut buf = vec![0u8; UDP_RECV_BUF_SIZE];

    loop {
        let (len, src_addr) = match socket.recv_from(&mut buf).await {
            Ok(r)  => r,
            Err(e) => { warn!("[media] recv_from: {}", e); continue; }
        };

        let packet = buf[..len].to_vec();
        trace!("[media] {} bytes from {}", len, src_addr);

        if packet.is_empty() { continue; }

        match classify(&packet) {
            PacketKind::Stun => {
                handle_stun(&socket, &packet, src_addr, &hub).await;
            }
            PacketKind::Dtls => {
                handle_dtls(
                    Arc::clone(&socket),
                    packet,
                    src_addr,
                    &hub,
                    Arc::clone(&cert),
                    Arc::clone(&session_map),
                ).await;
            }
            PacketKind::Srtp => {
                handle_srtp(&socket, &packet, src_addr, &hub).await;
            }
            PacketKind::Unknown => {
                debug!("[media] unknown packet type from {}", src_addr);
            }
        }
    }
}

// ----------------------------------------------------------------------------
// [STUN 핸들러] — 콜드패스
//
// STUN Binding Request:
//   USERNAME attribute = "서버ufrag:클라이언트ufrag"
//   클라이언트ufrag(콜론 뒤)로 by_ufrag 조회 → latch → Binding Response
// ----------------------------------------------------------------------------

async fn handle_stun(
    socket:   &UdpSocket,
    packet:   &[u8],
    src_addr: std::net::SocketAddr,
    hub:      &MediaPeerHub,
) {
    trace!("[stun] Binding Request from {}", src_addr);

    let ufrag = match parse_stun_username(packet) {
        Some(u) => u,
        None    => { debug!("[stun] no USERNAME, dropping"); return; }
    };

    let ep = match hub.latch(&ufrag, src_addr) {
        Some(ep) => { trace!("[stun] latched ufrag={} user={} addr={}", ufrag, ep.user_id, src_addr); ep }
        None     => { debug!("[stun] unknown ufrag={}, dropping", ufrag); return; }
    };

    // MESSAGE-INTEGRITY + FINGERPRINT 포함 Binding Response 전송
    // ice_pwd: 서버가 SDP answer에 넣어준 pwd → HMAC-SHA1 서명 키
    if let Some(resp) = make_binding_response(packet, src_addr, &ep.ice_pwd) {
        if let Err(e) = socket.send_to(&resp, src_addr).await {
            warn!("[stun] response failed: {}", e);
        } else {
            trace!("[stun] Binding Response sent to {}", src_addr);
        }
    }
}

// ----------------------------------------------------------------------------
// [DTLS 핸들러]
//
// 두 가지 경우:
//   1. 기존 세션 존재 → DtlsSessionMap.inject() 로 패킷 주입
//   2. 신규 세션 (ClientHello) → start_dtls_handshake() 로 핸드셰이크 시작
//
// latch 이후 by_addr로 Endpoint 조회 가능.
// STUN이 먼저 와서 latch를 끝냈다면 DTLS 도착 시 Endpoint가 존재함.
// ----------------------------------------------------------------------------

async fn handle_dtls(
    socket:      Arc<UdpSocket>,
    packet:      Vec<u8>,
    src_addr:    std::net::SocketAddr,
    hub:         &MediaPeerHub,
    cert:        Arc<ServerCert>,
    session_map: Arc<DtlsSessionMap>,
) {
    // 1. 기존 핸드셰이크 세션에 패킷 주입 (핫패스)
    if session_map.inject(&src_addr, packet.clone()).await {
        trace!("[dtls] injected {} bytes to existing session addr={}", packet.len(), src_addr);
        return;
    }

    // 2. 신규 세션 — Endpoint가 latch 돼 있어야 함
    let endpoint = match hub.get_by_addr(&src_addr) {
        Some(ep) => ep,
        None     => {
            debug!("[dtls] no endpoint for addr={}, waiting for STUN latch", src_addr);
            return;
        }
    };

    info!("[dtls] new session for user={} addr={}", endpoint.user_id, src_addr);

    // 3. UdpConnAdapter 생성 + 핸드셰이크 시작 (백그라운드)
    //    첫 번째 패킷(ClientHello)은 핸드셰이크 spawn 직후 주입
    start_dtls_handshake(
        socket,
        src_addr,
        endpoint,
        cert,
        Arc::clone(&session_map),
    ).await;

    // 4. 핸드셰이크 시작 후 첫 번째 패킷을 세션에 주입
    //    (start_dtls_handshake 내부에서 session_map에 등록 완료됨)
    if !session_map.inject(&src_addr, packet).await {
        warn!("[dtls] failed to inject initial packet addr={}", src_addr);
    }
}

// ----------------------------------------------------------------------------
// [SRTP 핸들러] — 핫패스
//
// by_addr O(1) 조회 → inbound 복호화 → 같은 채널 → outbound 재암호화 → 릴레이
// ----------------------------------------------------------------------------

async fn handle_srtp(
    socket:   &UdpSocket,
    packet:   &[u8],
    src_addr: std::net::SocketAddr,
    hub:      &MediaPeerHub,
) {
    let ep = match hub.get_by_addr(&src_addr) {
        Some(e) => e,
        None    => { debug!("[srtp] unknown addr={}, dropping", src_addr); return; }
    };

    ep.touch();

    // inbound 복호화
    let plaintext = {
        let mut ctx = ep.inbound_srtp.lock().unwrap();
        match ctx.decrypt(packet) {
            Ok(p)  => p,
            Err(e) => { warn!("[srtp] decrypt failed user={}: {}", ep.user_id, e); return; }
        }
    };

    relay_to_channel(socket, &plaintext, &ep.ufrag, &ep.channel_id, hub).await;
}

// ----------------------------------------------------------------------------
// [릴레이] 같은 채널의 다른 엔드포인트에게 재암호화 후 전송
// ----------------------------------------------------------------------------

async fn relay_to_channel(
    socket:       &UdpSocket,
    plaintext:    &[u8],
    sender_ufrag: &str,
    channel_id:   &str,
    hub:          &MediaPeerHub,
) {
    let targets = hub.get_channel_endpoints(channel_id);

    for target in targets {
        if target.ufrag == sender_ufrag { continue; }

        let addr = match target.get_address() {
            Some(a) => a,
            None    => { debug!("[relay] user={} no addr yet", target.user_id); continue; }
        };

        let encrypted = {
            let mut ctx = target.outbound_srtp.lock().unwrap();
            match ctx.encrypt(plaintext) {
                Ok(p)  => p,
                Err(e) => { warn!("[relay] encrypt failed user={}: {}", target.user_id, e); continue; }
            }
        };

        if let Err(e) = socket.send_to(&encrypted, addr).await {
            warn!("[relay] send failed user={} addr={}: {}", target.user_id, addr, e);
        } else {
            trace!("[relay] {} bytes → user={} addr={}", encrypted.len(), target.user_id, addr);
        }
    }
}

// ----------------------------------------------------------------------------
// [STUN 유틸]
// ----------------------------------------------------------------------------

/// STUN 패킷에서 USERNAME attribute 파싱
/// USERNAME = "서버ufrag:클라이언트ufrag" → 클라이언트ufrag 반환
fn parse_stun_username(packet: &[u8]) -> Option<String> {
    // STUN 헤더: 20바이트 (type 2 + length 2 + magic 4 + tx_id 12)
    if packet.len() < 20 { return None; }

    const USERNAME_TYPE: u16 = 0x0006;
    let mut offset = 20usize;

    while offset + 4 <= packet.len() {
        let attr_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let attr_len  = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]) as usize;
        offset += 4;

        if offset + attr_len > packet.len() { break; }

        if attr_type == USERNAME_TYPE {
            let username = std::str::from_utf8(&packet[offset..offset + attr_len]).ok()?;
            // USERNAME = "server_ufrag:client_ufrag"
            // 서버 ufrag(nth(0))로 MediaPeerHub 조회 — 서버가 생성한 16자 ufrag
            let server_ufrag = username.split(':').next().unwrap_or(username);
            return Some(server_ufrag.to_string());
        }

        // 4바이트 정렬 패딩 skip
        offset += (attr_len + 3) & !3;
    }

    None
}

/// STUN Binding Response 생성
/// XOR-MAPPED-ADDRESS + MESSAGE-INTEGRITY (HMAC-SHA1) + FINGERPRINT (CRC32) 포함
/// RFC 5389 필수 속성 — 빠지면 브라우저가 응답을 무시함
fn make_binding_response(
    request:  &[u8],
    src_addr: std::net::SocketAddr,
    ice_pwd:  &str,
) -> Option<Vec<u8>> {
    use hmac::{Hmac, Mac};
    use sha1::Sha1;

    if request.len() < 20 { return None; }

    const MAGIC: u32 = 0x2112A442;

    // 헤더 (20바이트): length는 나중에 채움
    let mut resp = Vec::with_capacity(80);
    resp.extend_from_slice(&[0x01, 0x01]);              // Binding Success Response
    resp.extend_from_slice(&[0x00, 0x00]);              // length placeholder
    resp.extend_from_slice(&[0x21, 0x12, 0xA4, 0x42]); // Magic Cookie
    resp.extend_from_slice(&request[8..20]);            // Transaction ID 복사

    // XOR-MAPPED-ADDRESS (12바이트)
    match src_addr {
        std::net::SocketAddr::V4(v4) => {
            resp.extend_from_slice(&[0x00, 0x20]); // attr type
            resp.extend_from_slice(&[0x00, 0x08]); // attr length = 8
            resp.push(0x00);                        // reserved
            resp.push(0x01);                        // family: IPv4
            let xor_port = src_addr.port() ^ (MAGIC >> 16) as u16;
            resp.extend_from_slice(&xor_port.to_be_bytes());
            let xor_ip = u32::from(*v4.ip()) ^ MAGIC;
            resp.extend_from_slice(&xor_ip.to_be_bytes());
        }
        std::net::SocketAddr::V6(_) => return None, // IPv6는 Phase 3
    }

    // MESSAGE-INTEGRITY (24바이트): HMAC-SHA1(key=ice_pwd, msg=헤더~여기직전)
    // RFC 5389: length 필드를 이 attribute가 끝나는 시점의 길이로 업데이트한 후 HMAC 계산
    let msg_integrity_len = (resp.len() - 20 + 24) as u16; // XOR-MAP(12) + MI(24) = 36
    resp[2] = (msg_integrity_len >> 8) as u8;
    resp[3] = (msg_integrity_len & 0xFF) as u8;

    let mut mac = Hmac::<Sha1>::new_from_slice(ice_pwd.as_bytes())
        .expect("HMAC 키 오류");
    mac.update(&resp);
    let hmac_bytes = mac.finalize().into_bytes();

    resp.extend_from_slice(&[0x00, 0x08]); // attr type: MESSAGE-INTEGRITY
    resp.extend_from_slice(&[0x00, 0x14]); // attr length = 20 (SHA1 크기)
    resp.extend_from_slice(&hmac_bytes);

    // FINGERPRINT (8바이트): CRC32(packet) XOR 0x5354554E
    // RFC 5389: length 필드를 FINGERPRINT가 끝나는 시점으로 업데이트 후 CRC 계산
    let fingerprint_len = (resp.len() - 20 + 8) as u16;
    resp[2] = (fingerprint_len >> 8) as u8;
    resp[3] = (fingerprint_len & 0xFF) as u8;

    let crc = crc32fast::hash(&resp) ^ 0x5354_554E;
    resp.extend_from_slice(&[0x80, 0x28]); // attr type: FINGERPRINT
    resp.extend_from_slice(&[0x00, 0x04]); // attr length = 4
    resp.extend_from_slice(&crc.to_be_bytes());

    // 최종 length 필드 업데이트 (헤더 20바이트 제외한 나머지)
    let final_len = (resp.len() - 20) as u16;
    resp[2] = (final_len >> 8) as u8;
    resp[3] = (final_len & 0xFF) as u8;

    Some(resp)
}

// ----------------------------------------------------------------------------
// [테스트]
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_stun() {
        assert!(matches!(classify(&[0x00]), PacketKind::Stun));
        assert!(matches!(classify(&[0x01]), PacketKind::Stun));
    }

    #[test]
    fn test_classify_dtls() {
        assert!(matches!(classify(&[0x16]), PacketKind::Dtls)); // Handshake
        assert!(matches!(classify(&[0x14]), PacketKind::Dtls)); // ChangeCipherSpec
    }

    #[test]
    fn test_classify_srtp() {
        assert!(matches!(classify(&[0x80]), PacketKind::Srtp));
        assert!(matches!(classify(&[0xFF]), PacketKind::Srtp));
    }

    #[test]
    fn test_classify_unknown() {
        assert!(matches!(classify(&[0x50]), PacketKind::Unknown));
        assert!(matches!(classify(&[]), PacketKind::Unknown));
    }
}
