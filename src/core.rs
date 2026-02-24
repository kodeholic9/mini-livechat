// author: kodeholic (powered by Claude)
// 네트워크 로직과 철저히 분리된, 순수 비즈니스 상태 관리 모듈입니다.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{trace, warn};

use crate::error::{LiveError, LiveResult};
use crate::media::srtp::SrtpContext;
use crate::utils::current_timestamp;

/// 브로드캐스트 송신자 타입 (직렬화된 GatewayPacket JSON)
pub type BroadcastTx = mpsc::Sender<String>;

// ----------------------------------------------------------------------------
// [UserHub] WS 세션 관리 + 라우팅 테이블
// IDENTIFY 시 등록, WS 종료 시 제거
// ----------------------------------------------------------------------------

pub struct User {
    pub tx:        BroadcastTx,
    pub last_seen: AtomicU64,   // 마지막 메시지 수신 시간 (좀비 세션 감지용)
}

impl User {
    pub fn new(tx: BroadcastTx) -> Self {
        Self {
            tx,
            last_seen: AtomicU64::new(current_timestamp()),
        }
    }

    pub fn touch(&self) {
        self.last_seen.store(current_timestamp(), Ordering::Relaxed);
    }
}

pub struct UserHub {
    users: RwLock<HashMap<String, Arc<User>>>,
}

impl UserHub {
    pub fn new() -> Self {
        trace!("Initializing UserHub");
        Self { users: RwLock::new(HashMap::new()) }
    }

    pub fn register(&self, user_id: &str, tx: BroadcastTx) -> Arc<User> {
        let user = Arc::new(User::new(tx));
        self.users.write().unwrap().insert(user_id.to_string(), Arc::clone(&user));
        trace!("User registered: {}", user_id);
        user
    }

    pub fn unregister(&self, user_id: &str) {
        self.users.write().unwrap().remove(user_id);
        trace!("User unregistered: {}", user_id);
    }

    pub fn get(&self, user_id: &str) -> Option<Arc<User>> {
        self.users.read().unwrap().get(user_id).cloned()
    }

    /// user_id 목록을 받아 각각의 tx로 패킷 전송
    /// exclude: 브로드캐스트에서 제외할 user_id (발신자 본인 등)
    pub async fn broadcast_to(&self, user_ids: &HashSet<String>, packet_json: &str, exclude: Option<&str>) {
        let txs: Vec<Arc<User>> = {
            let users = self.users.read().unwrap();
            user_ids.iter()
                .filter(|uid| exclude.map_or(true, |ex| ex != uid.as_str()))
                .filter_map(|uid| users.get(uid).cloned())
                .collect()
        };

        for user in txs {
            if user.tx.send(packet_json.to_string()).await.is_err() {
                warn!("Broadcast failed: rx closed");
            }
        }
    }

    /// 좀비 세션 목록 반환 (last_seen 기준)
    pub fn find_zombies(&self, timeout_ms: u64) -> Vec<String> {
        let now = current_timestamp();
        self.users.read().unwrap()
            .iter()
            .filter(|(_, u)| now.saturating_sub(u.last_seen.load(Ordering::Relaxed)) >= timeout_ms)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

// ----------------------------------------------------------------------------
// [ChannelHub] 채널 정의 + 멤버 목록 관리
// ----------------------------------------------------------------------------

pub struct Channel {
    pub channel_id: String,
    pub capacity:   usize,
    pub created_at: u64,
    pub members:    RwLock<HashSet<String>>,    // user_id
}

impl Channel {
    pub fn new(channel_id: String, capacity: usize) -> Self {
        trace!("Creating Channel: {}", channel_id);
        Self {
            channel_id,
            capacity,
            created_at: current_timestamp(),
            members:    RwLock::new(HashSet::new()),
        }
    }

    pub fn add_member(&self, user_id: &str) -> LiveResult<()> {
        let mut members = self.members.write().unwrap();
        if members.len() >= self.capacity {
            warn!("Channel {} is full", self.channel_id);
            return Err(LiveError::ChannelFull(self.channel_id.clone()));
        }
        if !members.insert(user_id.to_string()) {
            return Err(LiveError::AlreadyInChannel(self.channel_id.clone()));
        }
        trace!("Member {} joined Channel {}", user_id, self.channel_id);
        Ok(())
    }

    pub fn remove_member(&self, user_id: &str) {
        self.members.write().unwrap().remove(user_id);
        trace!("Member {} left Channel {}", user_id, self.channel_id);
    }

    pub fn get_members(&self) -> HashSet<String> {
        self.members.read().unwrap().clone()
    }

    pub fn member_count(&self) -> usize {
        self.members.read().unwrap().len()
    }
}

pub struct ChannelHub {
    pub channels: RwLock<HashMap<String, Arc<Channel>>>,
}

impl ChannelHub {
    pub fn new() -> Self {
        trace!("Initializing ChannelHub");
        Self { channels: RwLock::new(HashMap::new()) }
    }

    pub fn create(&self, channel_id: &str, capacity: usize) -> Arc<Channel> {
        let mut channels = self.channels.write().unwrap();
        let ch = channels.entry(channel_id.to_string()).or_insert_with(|| {
            Arc::new(Channel::new(channel_id.to_string(), capacity))
        });
        Arc::clone(ch)
    }

    pub fn get(&self, channel_id: &str) -> Option<Arc<Channel>> {
        self.channels.read().unwrap().get(channel_id).cloned()
    }

    pub fn remove(&self, channel_id: &str) -> bool {
        self.channels.write().unwrap().remove(channel_id).is_some()
    }
}

// ----------------------------------------------------------------------------
// [MediaPeerHub] 미디어 릴레이 핫패스 전용
//
// 키 설계 (Phase 2):
//   by_ufrag : ice-ufrag → Endpoint  (STUN 콜드패스 식별자, 불변)
//   by_addr  : SocketAddr → Endpoint (UDP 핸패스 캐시, NAT 리바인딩 시 갱신)
//
// ssrc는 라우팅 키가 아니라 Endpoint.tracks 내부 메타데이터.
// BUNDLE 환경에서 하나의 Endpoint에 audio/video/data ssrc가 복수 달림.
// ----------------------------------------------------------------------------

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
    pub user_id:    String,
    pub channel_id: String,
    pub last_seen:  AtomicU64,          // 좌비 피어 감지용

    // 핵패스 캐시: NAT 리바인딩 시 STUN에서 갱신
    pub address: Mutex<Option<SocketAddr>>,

    // BUNDLE 트랙 목록 (ssrc 기준, 피어당 복수)
    pub tracks: RwLock<Vec<Track>>,

    // DTLS/SRTP 컨텍스트 (피어당 1개, 모든 트랙 공유)
    // Phase 2에서 SrtpContext로 교체 예정
    pub inbound_srtp:  Mutex<SrtpContext>,
    pub outbound_srtp: Mutex<SrtpContext>,
}

impl Endpoint {
    pub fn new(ufrag: String, user_id: String, channel_id: String) -> Self {
        trace!("Endpoint::new ufrag={} user={} channel={}", ufrag, user_id, channel_id);
        Self {
            ufrag,
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
    // 핵패스 전용: UDP 패킷 수신 시 O(1) 엔드포인트 조회
    by_addr:  RwLock<HashMap<SocketAddr, Arc<Endpoint>>>,
    // 콜드패스: STUN USERNAME에서 ufrag 파싱 후 엔드포인트 확정
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
    pub fn insert(&self, ufrag: &str, user_id: &str, channel_id: &str) -> Arc<Endpoint> {
        let ep = Arc::new(Endpoint::new(
            ufrag.to_string(),
            user_id.to_string(),
            channel_id.to_string(),
        ));
        self.by_ufrag.write().unwrap().insert(ufrag.to_string(), Arc::clone(&ep));
        trace!("Endpoint inserted: ufrag={} user={} channel={}", ufrag, user_id, channel_id);
        ep
    }

    /// STUN 콜드패스: ufrag으로 엔드포인트 확정 + by_addr 콜드패스 갱신
    pub fn latch(&self, ufrag: &str, addr: SocketAddr) -> Option<Arc<Endpoint>> {
        let ep = self.by_ufrag.read().unwrap().get(ufrag).cloned()?;
        ep.latch_address(addr);
        self.by_addr.write().unwrap().insert(addr, Arc::clone(&ep));
        trace!("Endpoint latched: ufrag={} addr={}", ufrag, addr);
        Some(ep)
    }

    /// 핵패스: SocketAddr로 O(1) 엔드포인트 조회
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
            trace!("Endpoint removed: ufrag={}", ufrag);
        }
    }

    /// 체널 내 모든 엔드포인트 반환 (릴레이 대상 목록)
    pub fn get_channel_endpoints(&self, channel_id: &str) -> Vec<Arc<Endpoint>> {
        self.by_ufrag.read().unwrap()
            .values()
            .filter(|ep| ep.channel_id == channel_id)
            .cloned()
            .collect()
    }

    /// 좌비 피어 목록 반환 (last_seen 기준)
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
