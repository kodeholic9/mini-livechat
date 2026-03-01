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

use crate::core::{ChannelHub, FloorControlState, MediaPeerHub};
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

// RFC 7983 §7 demultiplexing:
//   [0,   3]  → STUN
//   [16,  19] → ZRTP  (무시)
//   [20,  63] → DTLS
//   [64, 127] → TURN  (무시)
//   [128,191] → RTP/RTCP  ← 0x80~0xBF
//   [192,255] → 기타
// 주의: 기존 코드는 0x80 이상 전체를 SRTP로 처리 — RTCP(0xC8~등)도 포함됨
#[inline]
fn classify(buf: &[u8]) -> PacketKind {
    match buf.first() {
        Some(b) if *b <= 3               => PacketKind::Stun,
        Some(b) if *b >= 20 && *b <= 63  => PacketKind::Dtls,
        Some(b) if *b >= 128             => PacketKind::Srtp,   // RTP+RTCP 모두
        _                                => PacketKind::Unknown,
    }
}

// ----------------------------------------------------------------------------
// [UDP 릴레이 서버]
// ----------------------------------------------------------------------------

pub async fn run_udp_relay(
    peer_hub:     Arc<MediaPeerHub>,
    channel_hub:  Arc<ChannelHub>,
    cert:         Arc<ServerCert>,
    session_map:  Arc<DtlsSessionMap>,
    udp_port:     u16,
    advertise_ip: Option<String>,
) {
    // advertise_ip: SDP candidate에 광고할 IP
    // None이면 라우팅 테이블 기반 자동 감지
    let adv_ip = advertise_ip.unwrap_or_else(|| crate::protocol::sdp::detect_local_ip());
    info!("[media] advertise IP: {}", adv_ip);

    // 라우팅 테이블에 광고 IP 저장 (이후 SDP answer 생성 시 사용)
    crate::protocol::set_advertise_ip(adv_ip);

    let addr   = format!("0.0.0.0:{}", udp_port);
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
        let kind = classify(&packet);
        trace!("[media] {} bytes from {} kind={:?} byte0=0x{:02x}", len, src_addr, kind, packet[0]);

        if packet.is_empty() { continue; }

        match kind {
            PacketKind::Stun => {
                handle_stun(
                    Arc::clone(&socket),
                    &packet,
                    src_addr,
                    &peer_hub,
                    Arc::clone(&cert),
                    Arc::clone(&session_map),
                ).await;
            }
            PacketKind::Dtls => {
                handle_dtls(
                    Arc::clone(&socket),
                    packet,
                    src_addr,
                    &peer_hub,
                    Arc::clone(&cert),
                    Arc::clone(&session_map),
                ).await;
            }
            PacketKind::Srtp => {
                handle_srtp(&socket, &packet, src_addr, &peer_hub, &channel_hub).await;
            }
            PacketKind::Unknown => {
                trace!("[media] unknown packet type from {} byte0=0x{:02x}", src_addr, packet[0]);
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
    socket:      Arc<UdpSocket>,
    packet:      &[u8],
    src_addr:    std::net::SocketAddr,
    hub:         &MediaPeerHub,
    cert:        Arc<ServerCert>,
    session_map: Arc<DtlsSessionMap>,
) {
    // 핸패스: 이미 latch된 addr이면 write lock 없이 touch() + Response만
    if let Some(ep) = hub.get_by_addr(&src_addr) {
        ep.touch();
        trace!("[stun] keepalive from known addr={} user={}", src_addr, ep.user_id);
        if let Some(resp) = make_binding_response(packet, src_addr, &ep.ice_pwd) {
            let _ = socket.send_to(&resp, src_addr).await;
        }
        return;
    }

    // 콜드패스: 최초 latch
    trace!("[stun] cold path Binding Request from {}", src_addr);

    let ufrag = match parse_stun_username(packet) {
        Some(u) => u,
        None    => { debug!("[stun] no USERNAME, dropping"); return; }
    };

    let ep = match hub.latch(&ufrag, src_addr) {
        Some(ep) => { trace!("[stun] latched ufrag={} user={} addr={}", ufrag, ep.user_id, src_addr); ep }
        None     => { debug!("[stun] unknown ufrag={}, dropping", ufrag); return; }
    };

    // MESSAGE-INTEGRITY + FINGERPRINT 포함 Binding Response 전송
    if let Some(resp) = make_binding_response(packet, src_addr, &ep.ice_pwd) {
        if let Err(e) = socket.send_to(&resp, src_addr).await {
            warn!("[stun] response failed: {}", e);
        } else {
            trace!("[stun] Binding Response sent to {}", src_addr);
        }
    }

    // latch 완료 후 pending DTLS 패킷이 있으면 핸드셰이크 시작
    // (DTLS가 STUN보다 먼저 도착한 경우 대비)
    let pending = session_map.drain_pending(&src_addr).await;
    if !pending.is_empty() {
        info!("[stun] draining {} pending DTLS packet(s) for user={} addr={}", pending.len(), ep.user_id, src_addr);
        // 이미 핸드셰이크가 진행 중이 아닌 경우에만 시작
        if !session_map.inject(&src_addr, pending[0].clone()).await {
            start_dtls_handshake(
                Arc::clone(&socket),
                src_addr,
                ep,
                cert,
                session_map,
                pending,
            ).await;
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
    // 1. 기존 핸드셰이크 세션에 패킷 주입 (했패스)
    if session_map.inject(&src_addr, packet.clone()).await {
        trace!("[dtls] injected {} bytes to existing session addr={}", packet.len(), src_addr);
        return;
    }

    // 2. 신규 세션 — Endpoint가 latch 돼 있어야 함
    let endpoint = match hub.get_by_addr(&src_addr) {
        Some(ep) => ep,
        None     => {
            // STUN latch 전에 DTLS가 먼저 도착한 경우 — pending 큐에 저장
            // latch 완료 시 handle_stun()에서 drain하여 핸드셰이크 시작
            debug!("[dtls] no endpoint yet for addr={}, queuing as pending", src_addr);
            session_map.enqueue_pending(src_addr, packet).await;
            return;
        }
    };

    info!("[dtls] new session for user={} addr={}", endpoint.user_id, src_addr);

    start_dtls_handshake(
        socket,
        src_addr,
        endpoint,
        cert,
        Arc::clone(&session_map),
        vec![packet],  // 첫 번째 패킷 직접 주입
    ).await;
}

// ----------------------------------------------------------------------------
// [SRTP 핸들러] — 핫패스
//
// by_addr O(1) 조회 → inbound 복호화 → 같은 채널 → outbound 재암호화 → 릴레이
// ----------------------------------------------------------------------------

async fn handle_srtp(
    socket:      &UdpSocket,
    packet:      &[u8],
    src_addr:    std::net::SocketAddr,
    hub:         &MediaPeerHub,
    channel_hub: &ChannelHub,
) {
    let b1 = packet.get(1).copied().unwrap_or(0);
    trace!("[srtp] enter addr={} len={} byte0=0x{:02x} byte1=0x{:02x}", src_addr, packet.len(), packet.first().unwrap_or(&0), b1);

    let ep = match hub.get_by_addr(&src_addr) {
        Some(e) => e,
        None    => { debug!("[srtp] unknown addr={}, dropping", src_addr); return; }
    };

    trace!("[srtp] ep found user={} channel={} srtp_ready={}", ep.user_id, ep.channel_id, ep.inbound_srtp.lock().unwrap().is_ready());

    ep.touch();

    // RTCP 판별 (RFC 5761)
    // byte1 = M(1bit) | PT(7bit)
    // PT 200~207 → RTCP (SR/RR/SDES/BYE/APP/...)
    // PT 0~127   → RTP
    // 주의: byte1 >= 0xC8 판별은 잘못됨 — RTP PT가 72~79(0xC8~0xCF)면 오판
    let pt = b1 & 0x7F;
    let is_rtcp = pt >= 72 && pt <= 79;  // RFC 5761 §4: RTCP PT range 200-207 = 0x48-0x4F (masked)
    trace!("[srtp] is_rtcp={} user={}", is_rtcp, ep.user_id);

    // MutexGuard를 블록으로 감싸서 await 진입 전에 반드시 drop
    // (std::sync::MutexGuard는 Send가 아니므로 tokio::spawn 안에서 await 넘지불가)
    enum DecryptResult { Rtcp, Rtp(Vec<u8>), Err }

    let result = {
        let mut ctx = ep.inbound_srtp.lock().unwrap();
        if !ctx.is_ready() {
            trace!("[srtp] key not yet installed, dropping packet user={}", ep.user_id);
            return;
        }
        if is_rtcp {
            match ctx.decrypt_rtcp(packet) {
                Ok(_)  => trace!("[srtcp] ok user={}", ep.user_id),
                Err(e) => trace!("[srtcp] decrypt failed user={}: {}", ep.user_id, e),
            }
            DecryptResult::Rtcp
        } else {
            match ctx.decrypt(packet) {
                Ok(p)  => DecryptResult::Rtp(p),
                Err(e) => {
                    let pt = packet.get(1).map(|b| b & 0x7F).unwrap_or(0);
                    warn!("[srtp] decrypt failed user={} byte0=0x{:02x} pt={} len={}: {}",
                        ep.user_id, packet[0], pt, packet.len(), e);
                    DecryptResult::Err
                }
            }
        }
        // MutexGuard drop here (블록 종료)
    };

    let plaintext = match result {
        DecryptResult::Rtcp | DecryptResult::Err => return,
        DecryptResult::Rtp(p) => p,
    };

    relay_to_channel(socket, &plaintext, &ep.user_id, &ep.ufrag, &ep.channel_id, hub, channel_hub).await;
}

// ----------------------------------------------------------------------------
// [릴레이] 같은 채널의 다른 엔드포인트에게 재암호화 후 전송
// ----------------------------------------------------------------------------

async fn relay_to_channel(
    socket:       &UdpSocket,
    plaintext:    &[u8],
    sender_user:  &str,
    sender_ufrag: &str,
    channel_id:   &str,
    hub:          &MediaPeerHub,
    channel_hub:  &ChannelHub,
) {
    // Floor Control 체크: sender가 현재 floor holder여야만 릴레이
    // Idle 상태이거나 다른 사람이 holder이면 패킷 드롭
    if let Some(ch) = channel_hub.get(channel_id) {
        let floor = ch.floor.lock().unwrap();
        let state        = &floor.state;
        let taken_by     = floor.floor_taken_by.as_deref().unwrap_or("none");
        let is_granted   = *state == FloorControlState::Taken
            && floor.floor_taken_by.as_deref() == Some(sender_user);
        trace!("[relay] floor check user={} state={:?} taken_by={} granted={}", sender_user, state, taken_by, is_granted);
        drop(floor);
        if !is_granted {
            trace!("[relay] floor not granted for user={}, dropping", sender_user);
            return;
        }
    } else {
        trace!("[relay] channel not found channel_id={}", channel_id);
        return;
    }

    let targets = hub.get_channel_endpoints(channel_id);

    for target in targets {
        if target.ufrag == sender_ufrag { continue; } // 자기 자신 제외

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
            trace!("[stun] USERNAME={} → server_ufrag={}", username, server_ufrag);
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
