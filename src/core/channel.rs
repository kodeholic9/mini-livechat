// author: kodeholic (powered by Claude)
// ChannelHub — 채널 정의 + 멤버 목록 관리

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use tracing::trace;

use crate::error::{LiveError, LiveResult};
use crate::utils::current_timestamp;

use super::floor::{FloorControl, FloorControlState};

pub struct Channel {
    pub channel_id: String,
    pub freq:       String,             // 주파수번호 4자리 (예: "0312")
    pub name:       String,             // 채널명 (이모지 포함 가능)
    pub capacity:   usize,
    pub created_at: u64,
    pub members:    RwLock<HashSet<String>>,    // user_id
    pub floor:      Mutex<FloorControl>,        // MBCP Floor Control 상태
}

impl Channel {
    pub fn new(channel_id: String, freq: String, name: String, capacity: usize) -> Self {
        trace!("Creating Channel: {} freq={} name={}", channel_id, freq, name);
        Self {
            channel_id,
            freq,
            name,
            capacity,
            created_at: current_timestamp(),
            members:    RwLock::new(HashSet::new()),
            floor:      Mutex::new(FloorControl::new()),
        }
    }

    pub fn add_member(&self, user_id: &str) -> LiveResult<()> {
        let mut members = self.members.write().unwrap();
        if members.len() >= self.capacity {
            tracing::warn!("Channel {} is full", self.channel_id);
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

    pub fn create(&self, channel_id: &str, freq: &str, name: &str, capacity: usize) -> Arc<Channel> {
        let mut channels = self.channels.write().unwrap();
        let ch = channels.entry(channel_id.to_string()).or_insert_with(|| {
            Arc::new(Channel::new(
                channel_id.to_string(),
                freq.to_string(),
                name.to_string(),
                capacity,
            ))
        });
        Arc::clone(ch)
    }

    pub fn get(&self, channel_id: &str) -> Option<Arc<Channel>> {
        self.channels.read().unwrap().get(channel_id).cloned()
    }

    pub fn remove(&self, channel_id: &str) -> bool {
        self.channels.write().unwrap().remove(channel_id).is_some()
    }

    /// 현재 채널 수
    pub fn count(&self) -> usize {
        self.channels.read().unwrap().len()
    }

    /// Floor Taken 상태인 채널 수
    pub fn count_floor_taken(&self) -> usize {
        self.channels.read().unwrap()
            .values()
            .filter(|ch| ch.floor.lock().unwrap().state == FloorControlState::Taken)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_get_channel() {
        let hub = ChannelHub::new();
        hub.create("CH_001", "0001", "test", 10);
        assert!(hub.get("CH_001").is_some());
        assert!(hub.get("CH_999").is_none());
    }

    #[test]
    fn create_duplicate_returns_existing() {
        let hub = ChannelHub::new();
        hub.create("CH_001", "0001", "first", 10);
        hub.create("CH_001", "0001", "second", 20);
        assert_eq!(hub.count(), 1);
        // or_insert_with — 첫 번째 값 유지
        assert_eq!(hub.get("CH_001").unwrap().capacity, 10);
    }

    #[test]
    fn remove_channel() {
        let hub = ChannelHub::new();
        hub.create("CH_001", "0001", "test", 10);
        assert!(hub.remove("CH_001"));
        assert!(!hub.remove("CH_001"));
        assert_eq!(hub.count(), 0);
    }

    #[test]
    fn add_member_and_count() {
        let hub = ChannelHub::new();
        let ch = hub.create("CH_001", "0001", "test", 10);
        ch.add_member("alice").unwrap();
        ch.add_member("bob").unwrap();
        assert_eq!(ch.member_count(), 2);
    }

    #[test]
    fn add_member_capacity_full() {
        let hub = ChannelHub::new();
        let ch = hub.create("CH_001", "0001", "test", 2);
        ch.add_member("a").unwrap();
        ch.add_member("b").unwrap();
        let err = ch.add_member("c").unwrap_err();
        assert!(matches!(err, LiveError::ChannelFull(_)));
    }

    #[test]
    fn add_member_duplicate() {
        let hub = ChannelHub::new();
        let ch = hub.create("CH_001", "0001", "test", 10);
        ch.add_member("alice").unwrap();
        let err = ch.add_member("alice").unwrap_err();
        assert!(matches!(err, LiveError::AlreadyInChannel(_)));
    }

    #[test]
    fn remove_member_and_get_members() {
        let hub = ChannelHub::new();
        let ch = hub.create("CH_001", "0001", "test", 10);
        ch.add_member("alice").unwrap();
        ch.add_member("bob").unwrap();
        ch.remove_member("alice");
        let members = ch.get_members();
        assert_eq!(members.len(), 1);
        assert!(members.contains("bob"));
    }

    #[test]
    fn count_floor_taken_initially_zero() {
        let hub = ChannelHub::new();
        hub.create("CH_001", "0001", "test", 10);
        hub.create("CH_002", "0002", "test2", 10);
        assert_eq!(hub.count_floor_taken(), 0);
    }
}
