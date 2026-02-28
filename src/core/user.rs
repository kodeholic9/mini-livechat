// author: kodeholic (powered by Claude)
// UserHub — WS 세션 관리 + 라우팅 테이블

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use tracing::{trace, warn};

use crate::utils::current_timestamp;

/// 브로드캐스트 송신자 타입 (직렬화된 GatewayPacket JSON)
pub type BroadcastTx = mpsc::Sender<String>;

// ----------------------------------------------------------------------------
// [User] IDENTIFY 시 등록, WS 종료 시 제거
// ----------------------------------------------------------------------------

pub struct User {
    pub tx:        BroadcastTx,
    pub last_seen: AtomicU64,   // 마지막 메시지 수신 시간 (좀비 세션 감지용)
    pub priority:  u8,          // Floor Control 우선순위 (MBCP, 높을수록 우선)
}

impl User {
    pub fn new(tx: BroadcastTx, priority: u8) -> Self {
        Self {
            tx,
            last_seen: AtomicU64::new(current_timestamp()),
            priority,
        }
    }

    pub fn touch(&self) {
        self.last_seen.store(current_timestamp(), Ordering::Relaxed);
    }
}

// ----------------------------------------------------------------------------
// [UserHub] 전역 라우팅 테이블
// ----------------------------------------------------------------------------

pub struct UserHub {
    users: RwLock<HashMap<String, Arc<User>>>,
}

impl UserHub {
    pub fn new() -> Self {
        trace!("Initializing UserHub");
        Self { users: RwLock::new(HashMap::new()) }
    }

    pub fn register(&self, user_id: &str, tx: BroadcastTx, priority: u8) -> Arc<User> {
        let user = Arc::new(User::new(tx, priority));
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

    /// 전체 User 목록 반환 (admin 조회용)
    pub fn all_users(&self) -> Vec<(String, Arc<User>)> {
        self.users.read().unwrap()
            .iter()
            .map(|(id, u)| (id.clone(), Arc::clone(u)))
            .collect()
    }

    /// 현재 접속 User 수
    pub fn count(&self) -> usize {
        self.users.read().unwrap().len()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tx() -> BroadcastTx {
        let (tx, _rx) = mpsc::channel(16);
        tx
    }

    #[test]
    fn register_and_get() {
        let hub = UserHub::new();
        hub.register("alice", make_tx(), 100);
        assert!(hub.get("alice").is_some());
        assert!(hub.get("bob").is_none());
    }

    #[test]
    fn unregister_removes_user() {
        let hub = UserHub::new();
        hub.register("alice", make_tx(), 100);
        hub.unregister("alice");
        assert!(hub.get("alice").is_none());
        assert_eq!(hub.count(), 0);
    }

    #[test]
    fn count_tracks_users() {
        let hub = UserHub::new();
        assert_eq!(hub.count(), 0);
        hub.register("a", make_tx(), 100);
        hub.register("b", make_tx(), 100);
        assert_eq!(hub.count(), 2);
        hub.unregister("a");
        assert_eq!(hub.count(), 1);
    }

    #[test]
    fn duplicate_register_overwrites() {
        let hub = UserHub::new();
        hub.register("alice", make_tx(), 50);
        hub.register("alice", make_tx(), 200);
        assert_eq!(hub.count(), 1);
        assert_eq!(hub.get("alice").unwrap().priority, 200);
    }

    #[test]
    fn all_users_returns_snapshot() {
        let hub = UserHub::new();
        hub.register("a", make_tx(), 100);
        hub.register("b", make_tx(), 200);
        let all = hub.all_users();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn touch_updates_last_seen() {
        let hub = UserHub::new();
        let user = hub.register("alice", make_tx(), 100);
        let t1 = user.last_seen.load(Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(5));
        user.touch();
        let t2 = user.last_seen.load(Ordering::Relaxed);
        assert!(t2 >= t1);
    }

    #[test]
    fn find_zombies_fresh_users_empty() {
        let hub = UserHub::new();
        hub.register("alice", make_tx(), 100);
        let zombies = hub.find_zombies(60_000);
        assert!(zombies.is_empty());
    }
}
