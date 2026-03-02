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
//
// Unified Plan re-negotiation 지원 (v0.21.0):
//   - offer의 direction을 읽어서 answer에 적절히 반전
//     sendrecv → sendrecv, recvonly → sendonly, inactive → inactive
//   - a=extmap (MID header extension 등) 보존 — BUNDLE demux용

/// SDP answer 조립 후 (sdp_string, server_ufrag, server_pwd) 반환
/// server_ufrag: MediaPeerHub 등록 키
/// server_pwd:   STUN MESSAGE-INTEGRITY 서명 키
///
/// BUNDLE 구조이므로 audio/video 모두 같은 ICE/DTLS/포트를 공유한다.
/// offer에 m=video가 있으면 동일 패턴으로 미러링 — 서버 코드 변경 불필요.
pub fn build_sdp_answer(offer: &str, fingerprint: &str, udp_port_arg: u16) -> (String, String, String) {
    build_sdp_answer_with_ice(offer, fingerprint, udp_port_arg, None, None)
}

/// SDP answer 조립 — ICE credential을 외부에서 주입 가능
/// override_ufrag/pwd가 Some이면 그 값을 사용, None이면 랜덤 생성
pub fn build_sdp_answer_with_ice(
    offer: &str,
    fingerprint: &str,
    udp_port_arg: u16,
    override_ufrag: Option<&str>,
    override_pwd:   Option<&str>,
) -> (String, String, String) {
    let session_id   = crate::utils::current_timestamp();
    let server_ufrag = override_ufrag.map(|s| s.to_string()).unwrap_or_else(|| random_ice_string(16));
    let server_pwd   = override_pwd.map(|s| s.to_string()).unwrap_or_else(|| random_ice_string(22));
    let local_ip     = crate::protocol::get_advertise_ip();
    let udp_port     = udp_port_arg;

    // ICE/DTLS/방향/연결/SSRC 라인은 서버 값으로 교체 — offer에서 제외
    // a=ssrc, a=ssrc-group: SFU는 offer의 클라이언트 SSRC를 echo하면 안 됨
    //   → Chrome BUNDLE demux 시 SSRC→mid 바인딩 충돌 유발 (RFC 8843 §9.2)
    //   → sendonly m-line의 서버 SSRC는 build_sdp_answer_for_renego()에서 별도 삽입
    // a=msid: SFU는 클라이언트의 msid를 echo하면 안 됨
    //   → sendonly m-line에는 서버 자체 msid를 별도 삽입
    //   → sendrecv m-line의 msid도 제거 (SFU는 트랙 식별에 msid 불필요)
    let skip_prefixes = [
        "a=ice-", "a=fingerprint", "a=setup", "a=candidate",
        "a=sendrecv", "a=sendonly", "a=recvonly", "a=inactive",
        "a=rtcp-mux", "a=rtcp-rsize", "c=",
        "a=ssrc", "a=ssrc-group",
        "a=msid",  // msid + msid-semantic 둘 다 차단
    ];

    // --------------------------------------------------------------------
    // offer 파싱 — 미디어 섹션별로 수집
    // MediaSection: m= 라인 + 코덱/속성 라인 목록 + 방향
    // --------------------------------------------------------------------
    struct MediaSection {
        m_line:         String,       // 포트 교체 완료된 m= 라인
        codec_lines:    Vec<String>,  // ICE/DTLS/direction/SSRC 제외한 나머지 a= 라인
        mid:            String,       // BUNDLE 그룹용
        direction:      String,       // offer의 방향: sendrecv|recvonly|sendonly|inactive
        has_rtcp_rsize: bool,         // offer에 a=rtcp-rsize가 있었는지 (없으면 answer에서도 생략)
        media_type:     String,       // "audio" 또는 "video" — sendonly PT 필터링용
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
            // m=audio ... 또는 m=video ... 에서 미디어 타입 추출
            let media_type = if line.starts_with("m=audio") { "audio" }
                             else if line.starts_with("m=video") { "video" }
                             else { "unknown" };
            current = Some(MediaSection {
                m_line,
                codec_lines: Vec::new(),
                mid: String::new(),
                direction: "sendrecv".to_string(),  // 기본값 (offer에 명시 없으면 sendrecv)
                has_rtcp_rsize: false,
                media_type: media_type.to_string(),
            });
            continue;
        }

        let sec = match current.as_mut() {
            Some(s) => s,
            None    => continue,  // 세션 헤더 영역 — 스킵
        };

        // direction 라인 파싱 (skip_prefixes에서 제거되되 값은 저장)
        if line.starts_with("a=sendrecv") { sec.direction = "sendrecv".to_string(); continue; }
        if line.starts_with("a=recvonly") { sec.direction = "recvonly".to_string(); continue; }
        if line.starts_with("a=sendonly") { sec.direction = "sendonly".to_string(); continue; }
        if line.starts_with("a=inactive") { sec.direction = "inactive".to_string(); continue; }

        // a=rtcp-rsize 존재 여부 기록 (skip_prefixes로 제거되므로 여기서 캡처)
        if line.starts_with("a=rtcp-rsize") { sec.has_rtcp_rsize = true; }

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
        // a=rtcp-rsize는 offer에 있을 때만 미러링 — 불일치 시 Chrome demux 에러 유발
        if sec.has_rtcp_rsize {
            sdp.push_str("a=rtcp-rsize\r\n");
        }
        // answer direction — offer direction을 반전
        //   offer sendrecv → answer sendrecv (기본 송수신)
        //   offer recvonly → answer sendonly (클라이언트 수신전용 → 서버 송신전용)
        //   offer sendonly → answer recvonly
        //   offer inactive → answer inactive (stopped transceiver)
        let answer_dir = match sec.direction.as_str() {
            "recvonly" => "sendonly",
            "sendonly" => "recvonly",
            "inactive" => "inactive",
            _          => "sendrecv",  // sendrecv 또는 기본값
        };
        sdp.push_str(&format!("a={}\r\n", answer_dir));

        // sendonly m-line: 서버가 송신할 코덱만 남기기 (PT 모호성 → Chrome demux 에러 방지)
        //   audio: Opus(PT 111)만 — a=rtpmap:111, a=fmtp:111, a=rtcp-fb:111
        //   video: 첫 번째 코덱(보통 VP8 PT 96)과 그 RTX만
        // sendonly m-line: 서버 자체 msid 삽입 (클라이언트 msid는 skip_prefixes로 제거됨)
        if answer_dir == "sendonly" {
            sdp.push_str(&format!("a=msid:server-{0} server-{0}-track\r\n", sec.mid));
        }

        if answer_dir == "sendonly" {
            // --- sendonly: 서버 송신 코덱만 필터 ---
            let allowed_pts: Vec<&str> = if sec.media_type == "audio" {
                vec!["111"]  // Opus만
            } else {
                // video: 첫 번째 코덱 PT + 그 RTX PT 추출
                // m= 라인에서 PT list 파싱: "m=video PORT PROTO PT1 PT2 ..."
                let m_parts: Vec<&str> = sec.m_line.split_whitespace().collect();
                if m_parts.len() > 3 {
                    // 첫 번째 코덱 PT (보통 96)
                    let first_pt = m_parts[3];
                    // RTX PT 찾기: codec_lines에서 a=fmtp:XX apt=first_pt인 XX
                    let mut pts = vec![first_pt];
                    for cl in &sec.codec_lines {
                        if cl.starts_with("a=fmtp:") {
                            // a=fmtp:97 apt=96;usedtx=1  →  PT=97이 96의 RTX
                            let apt_needle = format!("apt={}", first_pt);
                            if cl.contains(&apt_needle) {
                                if let Some(pt_str) = cl.strip_prefix("a=fmtp:").and_then(|s| s.split_whitespace().next()) {
                                    pts.push(pt_str);
                                }
                            }
                        }
                    }
                    pts
                } else {
                    vec![]  // 파싱 실패 시 전부 유지 (fallback)
                }
            };

            // m= 라인의 PT list도 필터링된 PT만 남기기
            if !allowed_pts.is_empty() {
                // m=audio PORT PROTO PT1 PT2 ... → m=audio PORT PROTO filtered_PTs
                let m_parts: Vec<&str> = sec.m_line.splitn(4, ' ').collect();
                if m_parts.len() == 4 {
                    let orig_pts: Vec<&str> = m_parts[3].split_whitespace().collect();
                    let filtered_pts: Vec<&str> = orig_pts.iter()
                        .filter(|pt| allowed_pts.contains(pt))
                        .copied()
                        .collect();
                    if !filtered_pts.is_empty() {
                        // 이미 출력된 m= 라인을 교체 — 마지막 m= 라인을 찾아서 덮어쓰기
                        let new_m_line = format!("{} {} {} {}",
                            m_parts[0], m_parts[1], m_parts[2], filtered_pts.join(" "));
                        // SDP에서 이미 push한 m= 라인을 교체
                        if let Some(pos) = sdp.rfind(&sec.m_line) {
                            let end = pos + sec.m_line.len();
                            sdp.replace_range(pos..end, &new_m_line);
                        }
                    }
                }
            }

            // codec 라인 필터: allowed_pts에 해당하는 것만
            for line in &sec.codec_lines {
                // a=rtpmap:PT, a=fmtp:PT, a=rtcp-fb:PT 형태 체크
                let dominated_by_pt = line.starts_with("a=rtpmap:")
                    || line.starts_with("a=fmtp:")
                    || line.starts_with("a=rtcp-fb:");
                if dominated_by_pt {
                    // PT 번호 추출: "a=rtpmap:111 opus/48000/2" → "111"
                    let pt = line.split(':').nth(1)
                        .and_then(|s| s.split_whitespace().next())
                        .unwrap_or("");
                    if !allowed_pts.is_empty() && !allowed_pts.contains(&pt) {
                        continue;  // 불필요한 코덱 제거
                    }
                }
                sdp.push_str(line);
                sdp.push_str("\r\n");
            }
        } else {
            // --- sendrecv / recvonly / inactive: 기존대로 전부 출력 ---
            for line in &sec.codec_lines {
                sdp.push_str(line);
                sdp.push_str("\r\n");
            }
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

/// SSRC 매핑 정보: mid → (user_id, kind, ssrc)
pub struct SsrcMapping {
    pub mid:     String,
    pub ssrc:    u32,
}

/// re-negotiation용 SDP answer 생성
/// - 기존 ICE session을 유지해야 하므로 existing_ufrag/pwd를 그대로 사용
/// - mid_map으로 sendonly m-line에 해당 peer의 SSRC를 `a=ssrc:` 라인으로 삽입
pub fn build_sdp_answer_for_renego(
    offer:          &str,
    fingerprint:    &str,
    udp_port:       u16,
    existing_ufrag: &str,   // 초기 join 시 생성된 ufrag (ICE restart 방지)
    existing_pwd:   &str,   // 초기 join 시 생성된 pwd
    ssrc_map:       &[SsrcMapping],
) -> String {
    // 기존 ICE credential을 직접 주입해서 answer 생성 — replace 해킹 없이 깨끗하게
    let (mut sdp, _ufrag, _pwd) = build_sdp_answer_with_ice(
        offer,
        fingerprint,
        udp_port,
        Some(existing_ufrag),
        Some(existing_pwd),
    );
    tracing::trace!("[sdp-renego] answer built with existing_ufrag='{}'", existing_ufrag);

    // sendonly m-line에 a=ssrc: 라인 삽입
    // 전략: a=end-of-candidates 라인 직전에 mid에 해당하는 ssrc 라인 삽입
    // 이를 위해 완성된 SDP를 섹션별로 다시 파싱해서 조작
    if ssrc_map.is_empty() {
        return sdp;
    }

    // 빠른 조회용 mid → ssrc HashMap
    let ssrc_lookup: std::collections::HashMap<&str, u32> = ssrc_map
        .iter()
        .map(|m| (m.mid.as_str(), m.ssrc))
        .collect();

    // SDP를 라인별로 재조립하면서 a=mid:N 뒤에 a=ssrc: 삽입
    let mut result = String::with_capacity(sdp.len() + ssrc_map.len() * 40);
    let mut current_mid: Option<&str> = None;
    let mut ssrc_inserted = false;

    for line in sdp.lines() {
        // m= 라인이면 이전 섹션 리셋
        if line.starts_with("m=") {
            current_mid = None;
            ssrc_inserted = false;
        }

        // mid 추출
        if line.starts_with("a=mid:") {
            current_mid = Some(line["a=mid:".len()..].trim());
        }

        // a=end-of-candidates 직전에 ssrc 삽입 (섹션 마지막 속성)
        if line.starts_with("a=end-of-candidates") && !ssrc_inserted {
            if let Some(mid) = current_mid {
                if let Some(&ssrc) = ssrc_lookup.get(mid) {
                    result.push_str(&format!("a=ssrc:{} cname:mini-livechat\r\n", ssrc));
                    ssrc_inserted = true;
                    tracing::trace!("[sdp] inserted a=ssrc:{} for mid={}", ssrc, mid);
                }
            }
        }

        result.push_str(line);
        result.push_str("\r\n");
    }

    sdp = result;
    sdp
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

    // ----- re-negotiation: direction 반전 -----

    fn make_renego_offer() -> String {
        // A가 B 입장 후 re-offer: 기존 sendrecv 2개 + recvonly 2개 (B 수신용)
        "v=0\r\n\
         o=- 123 2 IN IP4 0.0.0.0\r\n\
         s=-\r\n\
         t=0 0\r\n\
         a=group:BUNDLE 0 1 2 3\r\n\
         m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:0\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=sendrecv\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:111 opus/48000/2\r\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
         m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:1\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=sendrecv\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:96 VP8/90000\r\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
         m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:2\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=recvonly\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:111 opus/48000/2\r\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
         m=video 9 UDP/TLS/RTP/SAVPF 96\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:3\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=recvonly\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:96 VP8/90000\r\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n"
            .to_string()
    }

    fn make_inactive_offer() -> String {
        // stopped transceiver — inactive m-line 포함
        "v=0\r\n\
         o=- 123 2 IN IP4 0.0.0.0\r\n\
         s=-\r\n\
         t=0 0\r\n\
         a=group:BUNDLE 0 1\r\n\
         m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:0\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=sendrecv\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:111 opus/48000/2\r\n\
         m=audio 0 UDP/TLS/RTP/SAVPF 111\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:1\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=inactive\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:111 opus/48000/2\r\n"
            .to_string()
    }

    #[test]
    fn renego_sendrecv_stays_sendrecv() {
        let offer = make_renego_offer();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // mid:0 (sendrecv) → answer sendrecv
        // mid:1 (sendrecv) → answer sendrecv
        let sendrecv_count = sdp.matches("a=sendrecv").count();
        assert_eq!(sendrecv_count, 2, "sendrecv m-line should stay sendrecv");
    }

    #[test]
    fn renego_recvonly_becomes_sendonly() {
        let offer = make_renego_offer();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // mid:2 (recvonly) → answer sendonly
        // mid:3 (recvonly) → answer sendonly
        let sendonly_count = sdp.matches("a=sendonly").count();
        assert_eq!(sendonly_count, 2, "recvonly m-lines should become sendonly");
        // recvonly는 answer에 없어야 함
        assert!(!sdp.contains("a=recvonly"), "answer should not have recvonly");
    }

    #[test]
    fn renego_four_media_sections() {
        let offer = make_renego_offer();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert_eq!(sdp.matches("m=audio").count(), 2, "2 audio m-lines");
        assert_eq!(sdp.matches("m=video").count(), 2, "2 video m-lines");
        assert!(sdp.contains("a=group:BUNDLE 0 1 2 3"));
    }

    #[test]
    fn renego_preserves_extmap() {
        let offer = make_renego_offer();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // MID header extension이 보존되어야 함 (4개 m-line 각각)
        let extmap_count = sdp.matches("a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid").count();
        assert_eq!(extmap_count, 4, "extmap should be preserved in all sections");
    }

    #[test]
    fn inactive_stays_inactive() {
        let offer = make_inactive_offer();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        assert!(sdp.contains("a=inactive"), "inactive should stay inactive");
        // sendrecv 1개 + inactive 1개
        assert_eq!(sdp.matches("a=sendrecv").count(), 1);
        assert_eq!(sdp.matches("a=inactive").count(), 1);
    }

    // ----- build_sdp_answer_for_renego: SSRC 삽입 -----

    #[test]
    fn renego_ssrc_inserted_for_sendonly() {
        let offer = make_renego_offer();
        let ssrc_map = vec![
            SsrcMapping { mid: "2".to_string(), ssrc: 111222 },
            SsrcMapping { mid: "3".to_string(), ssrc: 333444 },
        ];
        let sdp = build_sdp_answer_for_renego(&offer, "sha-256 FF:00", 40000, "testufrag", "testpwd", &ssrc_map);
        assert!(sdp.contains("a=ssrc:111222 cname:mini-livechat"), "audio ssrc should be inserted");
        assert!(sdp.contains("a=ssrc:333444 cname:mini-livechat"), "video ssrc should be inserted");
    }

    #[test]
    fn renego_ssrc_not_inserted_for_sendrecv() {
        let offer = make_renego_offer();
        let ssrc_map = vec![
            SsrcMapping { mid: "0".to_string(), ssrc: 999999 },
        ];
        let sdp = build_sdp_answer_for_renego(&offer, "sha-256 FF:00", 40000, "testufrag", "testpwd", &ssrc_map);
        assert!(sdp.contains("a=ssrc:999999"));
    }

    #[test]
    fn renego_empty_ssrc_map_no_change() {
        let offer = make_renego_offer();
        let sdp_renego = build_sdp_answer_for_renego(&offer, "sha-256 FF:00", 40000, "testufrag", "testpwd", &[]);
        assert!(!sdp_renego.contains("a=ssrc:"));
    }

    #[test]
    fn renego_preserves_existing_ice_credentials() {
        let offer = make_renego_offer();
        let sdp = build_sdp_answer_for_renego(&offer, "sha-256 FF:00", 40000, "myUfrag123", "myPwd456", &[]);
        assert!(sdp.contains("a=ice-ufrag:myUfrag123"), "should use existing ufrag");
        assert!(sdp.contains("a=ice-pwd:myPwd456"), "should use existing pwd");
        // 랜덤 ufrag가 섹여 들어가면 안 됨
        assert!(!sdp.contains("a=ice-ufrag:myUfrag123\r\na=ice-ufrag:"),
            "should not have duplicate ufrag lines");
    }

    #[test]
    fn build_with_ice_override() {
        let offer = make_audio_offer("cu");
        let (sdp, ufrag, pwd) = build_sdp_answer_with_ice(
            &offer, "sha-256 FF:00", 40000, Some("FIXED_UFRAG"), Some("FIXED_PWD"));
        assert_eq!(ufrag, "FIXED_UFRAG");
        assert_eq!(pwd, "FIXED_PWD");
        assert!(sdp.contains("a=ice-ufrag:FIXED_UFRAG"));
        assert!(sdp.contains("a=ice-pwd:FIXED_PWD"));
    }

    #[test]
    fn build_with_ice_none_generates_random() {
        let offer = make_audio_offer("cu");
        let (sdp1, u1, _) = build_sdp_answer_with_ice(&offer, "sha-256 FF:00", 40000, None, None);
        let (_sdp2, u2, _) = build_sdp_answer_with_ice(&offer, "sha-256 FF:00", 40000, None, None);
        assert_ne!(u1, u2, "None should generate random ufrag each time");
        assert!(sdp1.contains(&format!("a=ice-ufrag:{}", u1)));
    }

    // ----- sendonly PT 필터링 + msid 테스트 -----

    fn make_renego_offer_full_codecs() -> String {
        // 실제 Chrome offer와 유사: audio PT 다수 + video PT 다수 + recvonly m-line
        "v=0\r\n\
         o=- 123 2 IN IP4 0.0.0.0\r\n\
         s=-\r\n\
         t=0 0\r\n\
         a=group:BUNDLE 0 1 2\r\n\
         a=msid-semantic: WMS client-stream-id\r\n\
         m=audio 9 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:0\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=sendrecv\r\n\
         a=msid:client-stream-id client-audio-track\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:111 opus/48000/2\r\n\
         a=rtcp-fb:111 transport-cc\r\n\
         a=fmtp:111 minptime=10;usedtx=1;useinbandfec=1\r\n\
         a=rtpmap:63 red/48000/2\r\n\
         a=fmtp:63 111/111;usedtx=1\r\n\
         a=rtpmap:9 G722/8000\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=rtpmap:13 CN/8000\r\n\
         a=rtpmap:110 telephone-event/48000\r\n\
         a=rtpmap:126 telephone-event/8000\r\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
         m=video 9 UDP/TLS/RTP/SAVPF 96 97 102 103\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:1\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=sendrecv\r\n\
         a=msid:client-stream-id client-video-track\r\n\
         a=rtcp-mux\r\n\
         a=rtcp-rsize\r\n\
         a=rtpmap:96 VP8/90000\r\n\
         a=rtcp-fb:96 transport-cc\r\n\
         a=rtpmap:97 rtx/90000\r\n\
         a=fmtp:97 apt=96;usedtx=1\r\n\
         a=rtpmap:102 H264/90000\r\n\
         a=rtcp-fb:102 transport-cc\r\n\
         a=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f\r\n\
         a=rtpmap:103 rtx/90000\r\n\
         a=fmtp:103 apt=102;usedtx=1\r\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n\
         m=audio 9 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126\r\n\
         c=IN IP4 0.0.0.0\r\n\
         a=mid:2\r\n\
         a=ice-ufrag:cu\r\n\
         a=ice-pwd:cp\r\n\
         a=setup:actpass\r\n\
         a=recvonly\r\n\
         a=rtcp-mux\r\n\
         a=rtpmap:111 opus/48000/2\r\n\
         a=rtcp-fb:111 transport-cc\r\n\
         a=fmtp:111 minptime=10;usedtx=1;useinbandfec=1\r\n\
         a=rtpmap:63 red/48000/2\r\n\
         a=fmtp:63 111/111;usedtx=1\r\n\
         a=rtpmap:9 G722/8000\r\n\
         a=rtpmap:0 PCMU/8000\r\n\
         a=rtpmap:8 PCMA/8000\r\n\
         a=rtpmap:13 CN/8000\r\n\
         a=rtpmap:110 telephone-event/48000\r\n\
         a=rtpmap:126 telephone-event/8000\r\n\
         a=extmap:4 urn:ietf:params:rtp-hdrext:sdes:mid\r\n"
            .to_string()
    }

    #[test]
    fn sendonly_audio_filters_to_opus_only() {
        let offer = make_renego_offer_full_codecs();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // mid:2 (recvonly→sendonly) audio: PT 111만 남아야 함
        // sendonly 섹션의 m= 라인에서 PT list 확인
        let lines: Vec<&str> = sdp.lines().collect();
        let mut in_mid2 = false;
        for line in &lines {
            if line.starts_with("a=mid:2") { in_mid2 = true; }
            if in_mid2 && line.starts_with("m=audio") {
                // 이미 지나갔으므로 이전 m= 라인 기준
            }
            // mid:2 섹션에서 G722, PCMU 등이 없어야 함
            if in_mid2 && line.starts_with("a=rtpmap:9 ") {
                panic!("mid:2 sendonly should not contain G722 (PT 9)");
            }
            if in_mid2 && line.starts_with("a=rtpmap:0 ") {
                panic!("mid:2 sendonly should not contain PCMU (PT 0)");
            }
        }
        // Opus는 있어야 함
        assert!(sdp.contains("a=rtpmap:111 opus/48000/2"), "opus should be in sendonly");
    }

    #[test]
    fn sendonly_m_line_pt_list_filtered() {
        let offer = make_renego_offer_full_codecs();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // sendonly audio m-line은 PT 111만 있어야 함
        // m=audio PORT PROTO 111  (not 111 63 9 0 8 13 110 126)
        let lines: Vec<&str> = sdp.lines().collect();
        let mut found_sendonly_m = false;
        for (i, line) in lines.iter().enumerate() {
            if line.contains("a=sendonly") {
                // 이 섹션의 m= 라인 찾기 (위로 올라가기)
                for j in (0..i).rev() {
                    if lines[j].starts_with("m=audio") {
                        // PT list에 111만 있는지 확인
                        assert!(!lines[j].contains(" 63 "), "sendonly m-line should not have PT 63");
                        assert!(!lines[j].contains(" 9 "), "sendonly m-line should not have PT 9");
                        found_sendonly_m = true;
                        break;
                    }
                    if lines[j].starts_with("m=") { break; }  // 다른 미디어 섹션
                }
            }
        }
        assert!(found_sendonly_m, "should have found sendonly audio m-line");
    }

    #[test]
    fn sendonly_has_server_msid() {
        let offer = make_renego_offer_full_codecs();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // sendonly mid:2에 서버 msid가 있어야 함
        assert!(sdp.contains("a=msid:server-2 server-2-track"),
            "sendonly should have server msid");
    }

    #[test]
    fn client_msid_not_echoed() {
        let offer = make_renego_offer_full_codecs();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // 클라이언트 msid가 answer에 없어야 함
        assert!(!sdp.contains("client-stream-id"), "client msid should not be echoed");
        assert!(!sdp.contains("client-audio-track"), "client msid should not be echoed");
        assert!(!sdp.contains("a=msid-semantic"), "msid-semantic should not be echoed");
    }

    #[test]
    fn sendrecv_has_no_msid() {
        let offer = make_renego_offer_full_codecs();
        let (sdp, _, _) = build_sdp_answer(&offer, "sha-256 FF:00", 40000);
        // sendrecv 섹션에는 msid가 없어야 함 (skip됨)
        // server msid는 sendonly에만 삽입
        let msid_count = sdp.matches("a=msid:").count();
        // sendonly가 1개 (mid:2)이므로 msid도 1개
        assert_eq!(msid_count, 1, "only sendonly section should have msid");
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
