// author: kodeholic (powered by Claude)
// UDP 미디어 릴레이 모듈
// 제어 평면(WebSocket)과 완전히 분리된 데이터 평면입니다.
//
// 흐름:
//   UdpSocket.recv_from()
//       → RTP 헤더에서 ssrc 파싱
//       → MediaPeerHub.get(ssrc)
//           → None  : 미등록 피어, 패킷 드롭
//           → Some  :
//               → update_address (Symmetric RTP Latching)
//               → srtp::decrypt (inbound)
//               → 채널 내 다른 피어들에게 srtp::encrypt + 릴레이

use std::sync::Arc;
use tokio::net::UdpSocket;
use tracing::{debug, trace, warn};

use crate::config;
use crate::core::MediaPeerHub;

// RTP 헤더 최소 크기 (fixed header 12 bytes)
const RTP_HEADER_MIN_LEN: usize = 12;
// SSRC 필드 오프셋 (bytes)
const RTP_SSRC_OFFSET: usize = 8;
// 수신 버퍼 크기
const UDP_RECV_BUF_SIZE: usize = 65535;

/// UDP 릴레이 서버 구동
/// lib.rs의 run_server()에서 tokio::spawn으로 호출됩니다.
pub async fn run_udp_relay(media_peer_hub: Arc<MediaPeerHub>) {
    let addr = format!("0.0.0.0:{}", config::SERVER_UDP_PORT);
    let socket = match UdpSocket::bind(&addr).await {
        Ok(s) => {
            tracing::info!("[media] UDP relay listening on {}", addr);
            Arc::new(s)
        }
        Err(e) => {
            tracing::error!("[media] Failed to bind UDP socket on {}: {}", addr, e);
            return;
        }
    };

    let mut buf = vec![0u8; UDP_RECV_BUF_SIZE];

    loop {
        let (len, src_addr) = match socket.recv_from(&mut buf).await {
            Ok(r)  => r,
            Err(e) => { warn!("[media] recv_from error: {}", e); continue; }
        };

        let packet = &buf[..len];
        trace!("[media] recv {} bytes from {}", len, src_addr);

        // RTP 헤더 길이 검증
        if packet.len() < RTP_HEADER_MIN_LEN {
            debug!("[media] packet too short ({} bytes), dropping", len);
            continue;
        }

        let ssrc = parse_ssrc(packet);

        let peer = match media_peer_hub.get(ssrc) {
            Some(p) => p,
            None    => { debug!("[media] unknown ssrc={}, dropping", ssrc); continue; }
        };

        // Symmetric RTP Latching
        peer.update_address(src_addr);

        let channel_id = peer.channel_id.clone();

        // inbound SRTP 복호화
        let plaintext = {
            let ctx = peer.inbound_srtp.lock().unwrap();
            match ctx.decrypt(packet) {
                Ok(p)  => p.to_vec(),
                Err(e) => { warn!("[media] decrypt failed ssrc={}: {}", ssrc, e); continue; }
            }
        };

        relay_to_channel(&socket, &plaintext, ssrc, &channel_id, &media_peer_hub).await;
    }
}

/// 복호화된 RTP를 채널 내 다른 피어들에게 재암호화 후 릴레이
async fn relay_to_channel(
    socket:         &UdpSocket,
    plaintext:      &[u8],
    sender_ssrc:    u32,
    channel_id:     &str,
    media_peer_hub: &MediaPeerHub,
) {
    let peers = media_peer_hub.get_channel_peers(channel_id);

    for target in peers {
        if target.ssrc == sender_ssrc {
            continue;
        }

        let addr = match *target.address.lock().unwrap() {
            Some(a) => a,
            None    => {
                debug!("[media] ssrc={} has no address yet, skipping", target.ssrc);
                continue;
            }
        };

        // outbound SRTP 암호화
        let encrypted = {
            let ctx = target.outbound_srtp.lock().unwrap();
            match ctx.encrypt(plaintext) {
                Ok(p)  => p.to_vec(),
                Err(e) => { warn!("[media] encrypt failed ssrc={}: {}", target.ssrc, e); continue; }
            }
        };

        if let Err(e) = socket.send_to(&encrypted, addr).await {
            warn!("[media] relay to ssrc={} addr={} failed: {}", target.ssrc, addr, e);
        } else {
            trace!("[media] relayed {} bytes to ssrc={} addr={}", encrypted.len(), target.ssrc, addr);
        }
    }
}

/// RTP 헤더에서 SSRC 파싱 (offset 8, big-endian u32)
#[inline]
fn parse_ssrc(packet: &[u8]) -> u32 {
    u32::from_be_bytes([
        packet[RTP_SSRC_OFFSET],
        packet[RTP_SSRC_OFFSET + 1],
        packet[RTP_SSRC_OFFSET + 2],
        packet[RTP_SSRC_OFFSET + 3],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ssrc() {
        let mut packet = vec![0u8; 12];
        packet[8]  = 0x00;
        packet[9]  = 0x01;
        packet[10] = 0xE2;
        packet[11] = 0x40;
        assert_eq!(parse_ssrc(&packet), 123456);
    }

    #[test]
    fn test_rtp_header_too_short() {
        let short = vec![0u8; RTP_HEADER_MIN_LEN - 1];
        assert!(short.len() < RTP_HEADER_MIN_LEN, "짧은 패킷은 드롭되어야 합니다.");
    }
}
