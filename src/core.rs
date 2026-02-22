// author: kodeholic (powered by Gemini)
// 네트워크 로직과 철저히 분리된, 순수 비즈니스 상태 관리 모듈입니다.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use tracing::{trace, warn}; 

use crate::config;
use crate::utils::current_timestamp; 
use crate::error::{LiveError, LiveResult};

pub struct SrtpContext {}

// ----------------------------------------------------------------------------
// [채널 상태] LiveChannel
// ----------------------------------------------------------------------------
pub struct LiveChannel {
    pub channel_id: String,
    pub active_peers: RwLock<HashMap<u32, Weak<LivePeer>>>,
    pub current_speaker: AtomicU32,
    pub last_activity: AtomicU64,
}

impl LiveChannel {
    pub fn new(channel_id: String) -> Self {
        trace!("Creating new LiveChannel: {}", channel_id);
        Self {
            channel_id,
            active_peers: RwLock::new(HashMap::new()),
            current_speaker: AtomicU32::new(0),
            last_activity: AtomicU64::new(current_timestamp()),
        }
    }

    pub fn add_peer(&self, ssrc: u32, weak_peer: Weak<LivePeer>) -> LiveResult<()> {
        let mut peers = self.active_peers.write().unwrap();
        
        if peers.len() >= config::MAX_PEERS_PER_CHANNEL {
            warn!("Capacity exceeded for channel {}. Cannot add peer {}", self.channel_id, ssrc);
            return Err(LiveError::ChannelFull(self.channel_id.clone()));
        }
        
        peers.insert(ssrc, weak_peer);
        trace!("Peer {} successfully joined channel {}", ssrc, self.channel_id);
        Ok(())
    }

    pub fn remove_peer(&self, ssrc: u32) {
        let mut peers = self.active_peers.write().unwrap();
        peers.remove(&ssrc);
        trace!("Peer {} removed from channel {}", ssrc, self.channel_id);
        
        if self.current_speaker.load(Ordering::Relaxed) == ssrc {
            self.current_speaker.store(0, Ordering::Relaxed);
            trace!("Speaker {} left channel {}, microphone reset", ssrc, self.channel_id);
        }
    }
}

// ----------------------------------------------------------------------------
// [유저 상태] LivePeer
// ----------------------------------------------------------------------------
pub struct LivePeer {
    pub member_id: String,
    pub ssrc: u32,
    pub live_channel: Arc<LiveChannel>,
    pub address: Mutex<Option<SocketAddr>>,
    pub inbound_srtp: Mutex<SrtpContext>,
    pub outbound_srtp: Mutex<SrtpContext>,
    pub last_activity: AtomicU64,
}

impl LivePeer {
    pub fn new(member_id: String, ssrc: u32, channel: Arc<LiveChannel>) -> Self {
        trace!("Initializing new LivePeer [Member: {}, SSRC: {}]", member_id, ssrc);
        Self {
            member_id,
            ssrc,
            live_channel: channel,
            address: Mutex::new(None),
            inbound_srtp: Mutex::new(SrtpContext {}),
            outbound_srtp: Mutex::new(SrtpContext {}),
            last_activity: AtomicU64::new(current_timestamp()),
        }
    }
}

// ----------------------------------------------------------------------------
// [글로벌 허브] 라우팅 테이블
// ----------------------------------------------------------------------------
pub struct LiveChannelHub {
    pub channels: RwLock<HashMap<String, Arc<LiveChannel>>>,
}

impl LiveChannelHub {
    pub fn new() -> Self {
        trace!("Initializing LiveChannelHub");
        Self { channels: RwLock::new(HashMap::new()) }
    }

    pub fn get_or_create(&self, channel_id: &str) -> Arc<LiveChannel> {
        if let Some(channel) = self.channels.read().unwrap().get(channel_id) {
            trace!("Channel {} found via read-lock", channel_id);
            return Arc::clone(channel);
        }

        let mut channels = self.channels.write().unwrap();
        let channel = channels.entry(channel_id.to_string()).or_insert_with(|| {
            trace!("Channel {} not found, creating new channel via write-lock", channel_id);
            Arc::new(LiveChannel::new(channel_id.to_string()))
        });
        
        Arc::clone(channel)
    }
}

pub struct LivePeerHub {
    pub peers: RwLock<HashMap<u32, Arc<LivePeer>>>,
}

impl LivePeerHub {
    pub fn new() -> Self {
        trace!("Initializing LivePeerHub");
        Self { peers: RwLock::new(HashMap::new()) }
    }

    pub fn join_channel(
        &self, 
        member_id: &str,
        ssrc: u32, 
        channel_id: &str, 
        channel_hub: &LiveChannelHub
    ) -> LiveResult<Arc<LivePeer>> {
        
        trace!("Join request: Member {} (SSRC {}) to channel {}", member_id, ssrc, channel_id);

        let channel = channel_hub.get_or_create(channel_id);
        let peer = Arc::new(LivePeer::new(member_id.to_string(), ssrc, Arc::clone(&channel)));

        channel.add_peer(ssrc, Arc::downgrade(&peer))?;

        self.peers.write().unwrap().insert(ssrc, Arc::clone(&peer));

        trace!("Member {} (SSRC {}) successfully registered", member_id, ssrc);
        Ok(peer)
    }
}