// author: kodeholic (powered by Claude)
// MediaPeerHub — 미디어 릴레이 핫패스 전용
//
// 키 설계 (Phase 2):
//   by_ufrag : ice-ufrag → Endpoint  (STUN 콜드패스 식별자, 불변)
//   by_addr  : SocketAddr → Endpoint (UDP 핫패스 캐시, NAT 리바인딩 시 갱신)
//
// ssrc는 라우팅 키가 아니라 Endpoint.tracks 내부 메타데이터.
// BUNDLE 환경에서 하나의 Endpoint에 audio/video/data ssrc가 복수 달림.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, trace};

use crate::media::srtp::SrtpContext;
use crate::utils::current_timestamp;

/// BUNDLE 트랙 종류
#[derive(Debug, Clone, PartialEq)]
pub enum TrackKind {
    Audio,
    Video,
    Data,
}

/// 트랙 메타데이터 (ssrc 기준)
pub struct Track {
    pub ssrc: u32,
    pub kind: TrackKind,
}

/// 피어당 엔드포인트 (Phase 2 확장 대비 필드 포함)
pub struct Endpoint {
    pub ufrag:      String,             // ICE ufrag — 주키, 불변
    pub ice_pwd:    String,             // ICE pwd — STUN MESSAGE-INTEGRITY 검증용
    pub user_id:    String,
    pub channel_id: String,
    pub last_seen:  AtomicU64,          // 좀비 피어 감지용

    // 핫패스 캐시: NAT 리바인딩 시 STUN에서 갱신
    pub address: Mutex<Option<SocketAddr>>,

    // BUNDLE 트랙 목록 (ssrc 기준, 피어당 복수)
    pub tracks: RwLock<Vec<Track>>,

    // DTLS/SRTP 컨텍스트 (피어당 1개, 모든 트랙 공유)
    pub inbound_srtp:  Mutex<SrtpContext>,
    pub outbound_srtp: Mutex<SrtpContext>,
}

impl Endpoint {
    pub fn new(ufrag: String, ice_pwd: String, user_id: String, channel_id: String) -> Self {
        trace!("Endpoint::new ufrag={} user={} channel={}", ufrag, user_id, channel_id);
        Self {
            ufrag,
            ice_pwd,
            user_id,
            channel_id,
            last_seen:     AtomicU64::new(current_timestamp()),
            address:       Mutex::new(None),
            tracks:        RwLock::new(Vec::new()),
            inbound_srtp:  Mutex::new(SrtpContext::new()),
            outbound_srtp: Mutex::new(SrtpContext::new()),
        }
    }

    pub fn touch(&self) {
        self.last_seen.store(current_timestamp(), Ordering::Relaxed);
    }

    /// STUN Latching: 확정된 소스 주소 갱신
    pub fn latch_address(&self, addr: SocketAddr) {
        *self.address.lock().unwrap() = Some(addr);
        self.touch();
    }

    pub fn get_address(&self) -> Option<SocketAddr> {
        *self.address.lock().unwrap()
    }

    /// 트랙 등록 (ssrc + 종류)
    pub fn add_track(&self, ssrc: u32, kind: TrackKind) {
        let mut tracks = self.tracks.write().unwrap();
        if !tracks.iter().any(|t| t.ssrc == ssrc) {
            tracks.push(Track { ssrc, kind });
            trace!("Track added: ssrc={} ufrag={}", ssrc, self.ufrag);
        }
    }
}

pub struct MediaPeerHub {
    by_addr:  RwLock<HashMap<SocketAddr, Arc<Endpoint>>>,
    by_ufrag: RwLock<HashMap<String, Arc<Endpoint>>>,
}

impl MediaPeerHub {
    pub fn new() -> Self {
        trace!("Initializing MediaPeerHub");
        Self {
            by_addr:  RwLock::new(HashMap::new()),
            by_ufrag: RwLock::new(HashMap::new()),
        }
    }

    /// WS CHANNEL_JOIN 시 등록 — ufrag는 SDP 교환 후 확정
    pub fn insert(&self, ufrag: &str, ice_pwd: &str, user_id: &str, channel_id: &str) -> Arc<Endpoint> {
        let ep = Arc::new(Endpoint::new(
            ufrag.to_string(),
            ice_pwd.to_string(),
            user_id.to_string(),
            channel_id.to_string(),
        ));
        self.by_ufrag.write().unwrap().insert(ufrag.to_string(), Arc::clone(&ep));
        trace!("Endpoint inserted: ufrag={} user={} channel={}", ufrag, user_id, channel_id);
        ep
    }

    /// STUN 콜드패스: ufrag으로 엔드포인트 확정 + by_addr 캐시 갱신
    pub fn latch(&self, ufrag: &str, addr: SocketAddr) -> Option<Arc<Endpoint>> {
        let ep = self.by_ufrag.read().unwrap().get(ufrag).cloned()?;
        ep.latch_address(addr);
        self.by_addr.write().unwrap().insert(addr, Arc::clone(&ep));
        trace!("Endpoint latched: ufrag={} addr={}", ufrag, addr);
        Some(ep)
    }

    /// 핫패스: SocketAddr로 O(1) 엔드포인트 조회
    pub fn get_by_addr(&self, addr: &SocketAddr) -> Option<Arc<Endpoint>> {
        self.by_addr.read().unwrap().get(addr).cloned()
    }

    /// 엔드포인트 제거 (WS 종료 또는 CHANNEL_LEAVE)
    pub fn remove(&self, ufrag: &str) {
        let ep = self.by_ufrag.write().unwrap().remove(ufrag);
        if let Some(ep) = ep {
            if let Some(addr) = ep.get_address() {
                self.by_addr.write().unwrap().remove(&addr);
            }
            debug!("Endpoint removed: ufrag={}", ufrag);
        }
    }

    /// ufrag으로 엔드포인트 조회 (admin 상세 조회용)
    pub fn get_by_ufrag(&self, ufrag: &str) -> Option<Arc<Endpoint>> {
        self.by_ufrag.read().unwrap().get(ufrag).cloned()
    }

    /// 전체 Endpoint 목록 반환 (admin 조회용)
    pub fn all_endpoints(&self) -> Vec<Arc<Endpoint>> {
        self.by_ufrag.read().unwrap().values().cloned().collect()
    }

    /// 현재 Endpoint 수
    pub fn count(&self) -> usize {
        self.by_ufrag.read().unwrap().len()
    }

    /// 채널 내 모든 엔드포인트 반환 (릴레이 대상 목록)
    pub fn get_channel_endpoints(&self, channel_id: &str) -> Vec<Arc<Endpoint>> {
        self.by_ufrag.read().unwrap()
            .values()
            .filter(|ep| ep.channel_id == channel_id)
            .cloned()
            .collect()
    }

    /// 좀비 피어 목록 반환 (last_seen 기준)
    pub fn find_zombies(&self, timeout_ms: u64) -> Vec<String> {
        let now = current_timestamp();
        self.by_ufrag.read().unwrap()
            .values()
            .filter(|ep| now.saturating_sub(ep.last_seen.load(Ordering::Relaxed)) >= timeout_ms)
            .map(|ep| ep.ufrag.clone())
            .collect()
    }
}

// 호환성 에일리어스 — 기존 코드가 MediaPeer를 참조하는 곳에서 컴파일 에러 방지
pub type MediaPeer = Endpoint;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn addr(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port)
    }

    #[test]
    fn insert_and_get_by_ufrag() {
        let hub = MediaPeerHub::new();
        hub.insert("ufrag1", "pwd1", "alice", "CH_001");
        assert!(hub.get_by_ufrag("ufrag1").is_some());
        assert!(hub.get_by_ufrag("ufrag_x").is_none());
    }

    #[test]
    fn latch_enables_by_addr_lookup() {
        let hub = MediaPeerHub::new();
        hub.insert("ufrag1", "pwd1", "alice", "CH_001");
        let a = addr(5000);
        assert!(hub.get_by_addr(&a).is_none());
        hub.latch("ufrag1", a);
        assert!(hub.get_by_addr(&a).is_some());
    }

    #[test]
    fn latch_unknown_ufrag_returns_none() {
        let hub = MediaPeerHub::new();
        assert!(hub.latch("unknown", addr(5000)).is_none());
    }

    #[test]
    fn remove_clears_both_maps() {
        let hub = MediaPeerHub::new();
        hub.insert("ufrag1", "pwd1", "alice", "CH_001");
        let a = addr(5000);
        hub.latch("ufrag1", a);
        hub.remove("ufrag1");
        assert!(hub.get_by_ufrag("ufrag1").is_none());
        assert!(hub.get_by_addr(&a).is_none());
        assert_eq!(hub.count(), 0);
    }

    #[test]
    fn get_channel_endpoints_filters_by_channel() {
        let hub = MediaPeerHub::new();
        hub.insert("u1", "p", "alice", "CH_001");
        hub.insert("u2", "p", "bob",   "CH_001");
        hub.insert("u3", "p", "carol", "CH_002");
        assert_eq!(hub.get_channel_endpoints("CH_001").len(), 2);
        assert_eq!(hub.get_channel_endpoints("CH_002").len(), 1);
        assert_eq!(hub.get_channel_endpoints("CH_999").len(), 0);
    }

    #[test]
    fn count_and_all_endpoints() {
        let hub = MediaPeerHub::new();
        assert_eq!(hub.count(), 0);
        hub.insert("u1", "p", "alice", "CH_001");
        hub.insert("u2", "p", "bob",   "CH_001");
        assert_eq!(hub.count(), 2);
        assert_eq!(hub.all_endpoints().len(), 2);
    }

    #[test]
    fn endpoint_add_track_dedup() {
        let ep = Endpoint::new("u".into(), "p".into(), "alice".into(), "CH".into());
        ep.add_track(1234, TrackKind::Audio);
        ep.add_track(1234, TrackKind::Audio); // 중복
        ep.add_track(5678, TrackKind::Video);
        assert_eq!(ep.tracks.read().unwrap().len(), 2);
    }

    #[test]
    fn endpoint_latch_address() {
        let ep = Endpoint::new("u".into(), "p".into(), "alice".into(), "CH".into());
        assert!(ep.get_address().is_none());
        ep.latch_address(addr(9000));
        assert_eq!(ep.get_address(), Some(addr(9000)));
    }

    #[test]
    fn find_zombies_fresh_empty() {
        let hub = MediaPeerHub::new();
        hub.insert("u1", "p", "alice", "CH_001");
        assert!(hub.find_zombies(60_000).is_empty());
    }
}
