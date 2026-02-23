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
// [MediaPeerHub] 미디어 릴레이 핫패스 전용 (ssrc 기반 O(1) 조회)
// ----------------------------------------------------------------------------

pub struct MediaPeer {
    pub ssrc:          u32,
    pub user_id:       String,      // ssrc → user_id 역매핑
    pub channel_id:    String,      // 릴레이 대상 채널 식별
    pub address:       Mutex<Option<SocketAddr>>,
    pub last_seen:     AtomicU64,   // UDP 패킷 마지막 수신 시간 (좀비 피어 감지용)
    pub inbound_srtp:  Mutex<SrtpContext>,
    pub outbound_srtp: Mutex<SrtpContext>,
}

impl MediaPeer {
    pub fn new(ssrc: u32, user_id: String, channel_id: String) -> Self {
        trace!("Initializing MediaPeer [user: {}, ssrc: {}]", user_id, ssrc);
        Self {
            ssrc,
            user_id,
            channel_id,
            address:       Mutex::new(None),
            last_seen:     AtomicU64::new(current_timestamp()),
            inbound_srtp:  Mutex::new(SrtpContext::new()),
            outbound_srtp: Mutex::new(SrtpContext::new()),
        }
    }

    pub fn touch(&self) {
        self.last_seen.store(current_timestamp(), Ordering::Relaxed);
    }

    pub fn update_address(&self, addr: SocketAddr) {
        *self.address.lock().unwrap() = Some(addr);
        self.touch();
    }
}

pub struct MediaPeerHub {
    pub by_ssrc: RwLock<HashMap<u32, Arc<MediaPeer>>>,
}

impl MediaPeerHub {
    pub fn new() -> Self {
        trace!("Initializing MediaPeerHub");
        Self { by_ssrc: RwLock::new(HashMap::new()) }
    }

    pub fn insert(&self, ssrc: u32, user_id: &str, channel_id: &str) -> Arc<MediaPeer> {
        let peer = Arc::new(MediaPeer::new(ssrc, user_id.to_string(), channel_id.to_string()));
        self.by_ssrc.write().unwrap().insert(ssrc, Arc::clone(&peer));
        trace!("MediaPeer inserted: ssrc={} user={} channel={}", ssrc, user_id, channel_id);
        peer
    }

    pub fn get(&self, ssrc: u32) -> Option<Arc<MediaPeer>> {
        self.by_ssrc.read().unwrap().get(&ssrc).cloned()
    }

    pub fn remove(&self, ssrc: u32) {
        self.by_ssrc.write().unwrap().remove(&ssrc);
        trace!("MediaPeer removed: ssrc={}", ssrc);
    }

    /// 채널 내 모든 MediaPeer 반환 (릴레이 대상 목록)
    pub fn get_channel_peers(&self, channel_id: &str) -> Vec<Arc<MediaPeer>> {
        self.by_ssrc.read().unwrap()
            .values()
            .filter(|p| p.channel_id == channel_id)
            .cloned()
            .collect()
    }

    /// 좀비 피어 목록 반환
    pub fn find_zombies(&self, timeout_ms: u64) -> Vec<u32> {
        let now = current_timestamp();
        self.by_ssrc.read().unwrap()
            .iter()
            .filter(|(_, p)| now.saturating_sub(p.last_seen.load(Ordering::Relaxed)) >= timeout_ms)
            .map(|(ssrc, _)| *ssrc)
            .collect()
    }
}
