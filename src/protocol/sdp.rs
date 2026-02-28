// author: kodeholic (powered by Claude)
// SDP Answer 생성 모듈
//
// 브라우저 offer를 파싱해서 필요한 라인만 추출 후 서버 answer를 조립합니다.
// webrtc-sdp 크레이트 대신 직접 조립 — 버전 호환성 문제 방지.
//
// answer 구조:
//   - offer의 미디어 라인(m=, a=rtpmap 등) 미러링
//   - 서버 ICE ufrag/pwd (랜덤 16/22자)
//   - 서버 DTLS fingerprint (ServerCert에서)
//   - a=setup:passive (서버는 항상 passive)

/// SDP answer 조립 후 (sdp_string, server_ufrag, server_pwd) 반환
/// server_ufrag: MediaPeerHub 등록 키
/// server_pwd:   STUN MESSAGE-INTEGRITY 서명 키
///
/// BUNDLE 구조이므로 audio/video 모두 같은 ICE/DTLS/포트를 공유한다.
/// offer에 m=video가 있으면 동일 패턴으로 미러링 — 서버 코드 변경 불필요.
pub fn build_sdp_answer(offer: &str, fingerprint: &str, udp_port_arg: u16) -> (String, String, String) {
    let session_id   = crate::utils::current_timestamp();
    let server_ufrag = random_ice_string(16);
    let server_pwd   = random_ice_string(22);
    let local_ip     = crate::protocol::get_advertise_ip();
    let udp_port     = udp_port_arg;

    // ICE/DTLS/방향/연결 라인은 서버 값으로 교체 — offer에서 제외
    let skip_prefixes = [
        "a=ice-", "a=fingerprint", "a=setup", "a=candidate",
        "a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive",
        "a=rtcp-mux", "a=rtcp-rsize", "c=",
    ];

    // --------------------------------------------------------------------
    // offer 파싱 — 미디어 섹션별로 수집
    // MediaSection: m= 라인 + 코덱/속성 라인 목록
    // --------------------------------------------------------------------
    struct MediaSection {
        m_line:      String,       // 포트 교체 완료된 m= 라인
        codec_lines: Vec<String>,  // ICE/DTLS 제외한 나머지 a= 라인
        mid:         String,       // BUNDLE 그룹용
    }

    let mut sections: Vec<MediaSection> = Vec::new();
    let mut current: Option<MediaSection> = None;

    for line in offer.lines() {
        if line.starts_with("m=") {
            // 이전 섹션 저장
            if let Some(sec) = current.take() {
                sections.push(sec);
            }
            // 새 섹션 시작 — 포트만 서버 포트로 교체
            let parts: Vec<&str> = line.splitn(4, ' ').collect();
            let m_line = if parts.len() == 4 {
                format!("{} {} {} {}", parts[0], udp_port, parts[2], parts[3])
            } else {
                line.to_string()
            };
            current = Some(MediaSection {
                m_line,
                codec_lines: Vec::new(),
                mid: String::new(),
            });
            continue;
        }

        let sec = match current.as_mut() {
            Some(s) => s,
            None    => continue,  // 세션 헤더 영역 — 스킵
        };

        if skip_prefixes.iter().any(|p| line.starts_with(p)) { continue; }

        if line.starts_with("a=mid:") {
            sec.mid = line["a=mid:".len()..].trim().to_string();
        }
        sec.codec_lines.push(line.to_string());
    }
    if let Some(sec) = current.take() {
        sections.push(sec);
    }

    // BUNDLE 그룹: offer에 있는 mid 목록 순서대로
    let bundle_mids: Vec<&str> = sections.iter().map(|s| s.mid.as_str()).collect();

    // --------------------------------------------------------------------
    // answer 조립
    // --------------------------------------------------------------------
    let mut sdp = String::new();

    // 세션 헤더
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o=mini-livechat {0} {0} IN IP4 {1}\r\n", session_id, local_ip));
    sdp.push_str("s=-\r\n");
    sdp.push_str("t=0 0\r\n");
    sdp.push_str(&format!("a=group:BUNDLE {}\r\n", bundle_mids.join(" ")));
    sdp.push_str("a=ice-lite\r\n");

    // 미디어 섹션 — offer의 audio/video 순서대로 미러링
    for sec in &sections {
        sdp.push_str(&sec.m_line);
        sdp.push_str("\r\n");
        sdp.push_str(&format!("c=IN IP4 {}\r\n", local_ip));
        sdp.push_str(&format!("a=ice-ufrag:{}\r\n", server_ufrag));
        sdp.push_str(&format!("a=ice-pwd:{}\r\n", server_pwd));
        sdp.push_str(&format!("a=fingerprint:{}\r\n", fingerprint));
        sdp.push_str("a=setup:passive\r\n");
        sdp.push_str("a=rtcp-mux\r\n");
        sdp.push_str("a=rtcp-rsize\r\n");
        // sendrecv: recvonly 시 일부 브라우저가 DTLS를 시작하지 않는 문제 방지
        // 실제 미디어 방향은 Floor Control(애플리케이션 레이어)에서 제어
        sdp.push_str("a=sendrecv\r\n");
        for line in &sec.codec_lines {
            sdp.push_str(line);
            sdp.push_str("\r\n");
        }
        // ICE Lite — host candidate 1개
        sdp.push_str(&format!(
            "a=candidate:1 1 udp 2113937151 {} {} typ host generation 0\r\n",
            local_ip, udp_port
        ));
        sdp.push_str("a=end-of-candidates\r\n");
    }

    (sdp, server_ufrag, server_pwd)
}

/// 라우팅 테이블 기반 로컬 IP 자동 감지
/// UDP 소켓으로 8.8.8.8:80 connect (실제 패킷 없음) → local_addr() 조회
/// 멀티홈 환경에서도 외부 통신에 실제로 쓰이는 인터페이스 IP가 정확히 반환됨
pub fn detect_local_ip() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| { s.connect("8.8.8.8:80")?; s.local_addr() })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| {
            tracing::warn!("로컬 IP 감지 실패 — 127.0.0.1 폴백");
            "127.0.0.1".to_string()
        })
}

/// ICE ufrag/pwd용 랜덤 문자열 생성 (alphanumeric)
/// - rand 크레이트 기반 CSPRNG 사용 (xorshift 대비 충돌 안전)
/// - ufrag: 16자 권장 (RFC 8445 범위 4~256, 62^16 ≈ 4.7×10^28)
/// - pwd:   22자 (RFC 최솟값 준수)
pub fn random_ice_string(len: usize) -> String {
    use rand::Rng;
    let charset: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| charset[rng.gen_range(0..charset.len())] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- random_ice_string -----

    #[test]
    fn ice_string_length() {
        assert_eq!(random_ice_string(16).len(), 16);
        assert_eq!(random_ice_string(22).len(), 22);
        assert_eq!(random_ice_string(0).len(), 0);
    }

    #[test]
    fn ice_string_alphanumeric_only() {
        let s = random_ice_string(100);
        assert!(s.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn ice_string_unique() {
        let a = random_ice_string(16);
        let b = random_ice_string(16);
        // 62^16 공간에서 충돌할 확률 ~0
        assert_ne!(a, b);
    }

    // ----- build_sdp_answer -----

    fn make_audio_offer(ufrag: &str) -> String {
        format!(
            "v=0\r\n\
             o=- 123 2 IN IP4 0.0.0.0\r\n\
             s=-\r\n\
             t=0 0\r\n\
             a=group:BUNDLE 0\r\n\
             m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
             c=IN IP4 0.0.0.0\r\n\
             a=mid:0\r\n\
             a=ice-ufrag:{}\r\n\
             a=ice-pwd:clientpwd\r\n\
             a=fingerprint:sha-256 AA:BB\r\n\
             a=setup:actpass\r\n\
             a=sendrecv\r\n\
             a=rtcp-mux\r\n\
             a=rtpmap:111 opus/48000/2\r\n",
            ufrag
        )
    }

    fn make_bundle_offer() -> String {
        "v=0\r\n\
         o=- 123 2 IN IP4 0.0.0.0\r\n\
         s=-\r\n\
         t=0 0\r\n\
         a=group:BUNDLE 0 1\r\n\
         m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:0\r\n\
         a=ice-ufrag:cufrag\r\n\
         a=ice-pwd:cpwd\r\n\
         a=setup:actpass\r\n\
         a=sendrecv\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:111 opus/48000/2\r\n\
         m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:1\r\n\
         a=ice-ufrag:cufrag\r\n\
         a=ice-pwd:cpwd\r\n\
         a=setup:actpass\r\n\
         a=sendrecv\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:96 VP8/90000\r\n"
            .to_string()
    }

    #[test]
    fn answer_contains_server_ufrag_and_pwd() {
        let offer = make_audio_offer("clientufrag");
        let (sdp, ufrag, pwd) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert!(sdp.contains(&format!("a=ice-ufrag:{}", ufrag)));
        assert!(sdp.contains(&format!("a=ice-pwd:{}", pwd)));
    }

    #[test]
    fn answer_has_passive_setup() {
        let offer = make_audio_offer("cu");
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert!(sdp.contains("a=setup:passive"));
        // offer의 actpass가 남아있으면 안 됨
        assert!(!sdp.contains("actpass"));
    }

    #[test]
    fn answer_replaces_port() {
        let offer = make_audio_offer("cu");
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 41234);
        assert!(sdp.contains("m=audio 41234 "));
    }

    #[test]
    fn answer_includes_server_fingerprint() {
        let offer = make_audio_offer("cu");
        let fp = "sha-256 AB:CD:EF";
        let (sdp, _, _) = build_sdp_answer(&offer, fp, 40000);
        assert!(sdp.contains(&format!("a=fingerprint:{}", fp)));
        // offer의 fingerprint는 제거되어야 함
        assert!(!sdp.contains("AA:BB"));
    }

    #[test]
    fn answer_has_ice_lite() {
        let offer = make_audio_offer("cu");
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert!(sdp.contains("a=ice-lite"));
    }

    #[test]
    fn answer_has_host_candidate() {
        let offer = make_audio_offer("cu");
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert!(sdp.contains("typ host"));
        assert!(sdp.contains("a=end-of-candidates"));
    }

    #[test]
    fn answer_mirrors_codec_lines() {
        let offer = make_audio_offer("cu");
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert!(sdp.contains("a=rtpmap:111 opus/48000/2"));
    }

    #[test]
    fn answer_strips_client_ice_and_candidates() {
        let offer = make_audio_offer("clientufrag");
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert!(!sdp.contains("clientufrag"));
        assert!(!sdp.contains("clientpwd"));
    }

    #[test]
    fn bundle_offer_produces_two_media_sections() {
        let offer = make_bundle_offer();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        let m_audio_count = sdp.matches("m=audio").count();
        let m_video_count = sdp.matches("m=video").count();
        assert_eq!(m_audio_count, 1);
        assert_eq!(m_video_count, 1);
        assert!(sdp.contains("a=group:BUNDLE 0 1"));
    }

    #[test]
    fn bundle_answer_has_shared_ice_credentials() {
        let offer = make_bundle_offer();
        let (sdp, ufrag, pwd) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        let ufrag_count = sdp.matches(&format!("a=ice-ufrag:{}", ufrag)).count();
        let pwd_count   = sdp.matches(&format!("a=ice-pwd:{}", pwd)).count();
        // audio + video = 2개씩
        assert_eq!(ufrag_count, 2);
        assert_eq!(pwd_count, 2);
    }

    // ----- detect_local_ip -----

    #[test]
    fn detect_local_ip_returns_valid_ip() {
        let ip = detect_local_ip();
        assert!(!ip.is_empty());
        // 파싱 가능한 IP 주소여야 함
        assert!(ip.parse::<std::net::IpAddr>().is_ok());
    }
}
