// author: kodeholic (powered by Claude)

use mini_livechat::core::{ChannelHub, MediaPeerHub, UserHub};
use mini_livechat::config;
use tokio::sync::mpsc;

// ----------------------------------------------------------------------------
// [UserHub 테스트]
// ----------------------------------------------------------------------------

#[test]
fn test_user_register_and_unregister() {
    let hub = UserHub::new();
    let (tx, _rx) = mpsc::channel(10);

    hub.register("user_1", tx);
    assert!(hub.get("user_1").is_some(), "등록 후 조회가 가능해야 합니다.");

    hub.unregister("user_1");
    assert!(hub.get("user_1").is_none(), "해제 후 조회가 없어야 합니다.");
}

#[test]
fn test_user_touch_updates_last_seen() {
    let hub = UserHub::new();
    let (tx, _rx) = mpsc::channel(10);

    let user = hub.register("user_1", tx);
    let before = user.last_seen.load(std::sync::atomic::Ordering::Relaxed);

    std::thread::sleep(std::time::Duration::from_millis(10));
    user.touch();

    let after = user.last_seen.load(std::sync::atomic::Ordering::Relaxed);
    assert!(after > before, "touch 후 last_seen이 갱신되어야 합니다.");
}

#[test]
fn test_user_zombie_detection() {
    let hub = UserHub::new();
    let (tx, _rx) = mpsc::channel(10);

    hub.register("user_1", tx);

    // timeout을 0으로 설정하면 모두 좀비로 감지
    let zombies = hub.find_zombies(0);
    assert!(zombies.contains(&"user_1".to_string()), "타임아웃 초과 시 좀비로 감지되어야 합니다.");

    // 충분한 timeout이면 좀비 없음
    let zombies = hub.find_zombies(u64::MAX);
    assert!(zombies.is_empty(), "타임아웃 이내면 좀비가 없어야 합니다.");
}

// ----------------------------------------------------------------------------
// [ChannelHub 테스트]
// ----------------------------------------------------------------------------

#[test]
fn test_channel_create_and_get() {
    let hub = ChannelHub::new();

    hub.create("CH_1", 10);
    assert!(hub.get("CH_1").is_some(), "생성 후 조회가 가능해야 합니다.");

    assert!(hub.remove("CH_1"),        "삭제가 성공해야 합니다.");
    assert!(hub.get("CH_1").is_none(), "삭제 후 조회가 없어야 합니다.");
    assert!(!hub.remove("CH_1"),       "없는 채널 삭제는 false여야 합니다.");
}

#[test]
fn test_channel_add_and_remove_member() {
    let hub = ChannelHub::new();
    let ch  = hub.create("CH_1", 10);

    assert!(ch.add_member("user_1").is_ok());
    assert!(ch.get_members().contains("user_1"), "멤버가 채널에 있어야 합니다.");
    assert_eq!(ch.member_count(), 1);

    ch.remove_member("user_1");
    assert!(!ch.get_members().contains("user_1"), "멤버가 제거되어야 합니다.");
    assert_eq!(ch.member_count(), 0);
}

#[test]
fn test_channel_capacity_limit() {
    let hub = ChannelHub::new();
    let ch  = hub.create("CH_FULL", 3);

    assert!(ch.add_member("user_1").is_ok());
    assert!(ch.add_member("user_2").is_ok());
    assert!(ch.add_member("user_3").is_ok());

    let result = ch.add_member("user_4");
    assert!(result.is_err(), "정원 초과 시 에러가 발생해야 합니다.");
    assert!(result.unwrap_err().to_string().contains("full"), "ChannelFull 에러여야 합니다.");
}

#[test]
fn test_channel_duplicate_member() {
    let hub = ChannelHub::new();
    let ch  = hub.create("CH_1", 10);

    assert!(ch.add_member("user_1").is_ok());

    let result = ch.add_member("user_1");
    assert!(result.is_err(), "중복 입장은 거절되어야 합니다.");
    assert!(result.unwrap_err().to_string().contains("Already"), "AlreadyInChannel 에러여야 합니다.");
}

#[test]
fn test_channel_created_at_is_set() {
    let hub = ChannelHub::new();
    let ch  = hub.create("CH_1", 10);
    assert!(ch.created_at > 0, "created_at이 설정되어야 합니다.");
}

// ----------------------------------------------------------------------------
// [MediaPeerHub 테스트]
// ----------------------------------------------------------------------------

#[test]
fn test_media_peer_insert_and_get() {
    let hub = MediaPeerHub::new();

    hub.insert(100, "user_1", "CH_1");
    let peer = hub.get(100);
    assert!(peer.is_some(), "삽입 후 조회가 가능해야 합니다.");

    let peer = peer.unwrap();
    assert_eq!(peer.ssrc,       100);
    assert_eq!(peer.user_id,    "user_1");
    assert_eq!(peer.channel_id, "CH_1");
}

#[test]
fn test_media_peer_remove() {
    let hub = MediaPeerHub::new();

    hub.insert(100, "user_1", "CH_1");
    hub.remove(100);
    assert!(hub.get(100).is_none(), "제거 후 조회가 없어야 합니다.");
}

#[test]
fn test_media_peer_get_channel_peers() {
    let hub = MediaPeerHub::new();

    hub.insert(101, "user_1", "CH_1");
    hub.insert(102, "user_2", "CH_1");
    hub.insert(103, "user_3", "CH_2");

    let ch1_peers = hub.get_channel_peers("CH_1");
    assert_eq!(ch1_peers.len(), 2, "CH_1에 2명이 있어야 합니다.");

    let ch2_peers = hub.get_channel_peers("CH_2");
    assert_eq!(ch2_peers.len(), 1, "CH_2에 1명이 있어야 합니다.");
}

#[test]
fn test_media_peer_touch_updates_last_seen() {
    let hub  = MediaPeerHub::new();
    let peer = hub.insert(100, "user_1", "CH_1");

    let before = peer.last_seen.load(std::sync::atomic::Ordering::Relaxed);
    std::thread::sleep(std::time::Duration::from_millis(10));
    peer.touch();
    let after = peer.last_seen.load(std::sync::atomic::Ordering::Relaxed);

    assert!(after > before, "touch 후 last_seen이 갱신되어야 합니다.");
}

#[test]
fn test_media_peer_zombie_detection() {
    let hub = MediaPeerHub::new();
    hub.insert(100, "user_1", "CH_1");

    let zombies = hub.find_zombies(0);
    assert!(zombies.contains(&100), "타임아웃 초과 시 좀비로 감지되어야 합니다.");

    let zombies = hub.find_zombies(u64::MAX);
    assert!(zombies.is_empty(), "타임아웃 이내면 좀비가 없어야 합니다.");
}

#[test]
fn test_media_peer_update_address() {
    use std::net::SocketAddr;

    let hub  = MediaPeerHub::new();
    let peer = hub.insert(100, "user_1", "CH_1");

    assert!(peer.address.lock().unwrap().is_none(), "초기 주소는 None이어야 합니다.");

    let addr: SocketAddr = "127.0.0.1:5000".parse().unwrap();
    peer.update_address(addr);

    assert_eq!(*peer.address.lock().unwrap(), Some(addr), "주소 갱신이 되어야 합니다.");
}

// ----------------------------------------------------------------------------
// [전체 흐름 통합 테스트]
// IDENTIFY → CHANNEL_CREATE → CHANNEL_JOIN → MESSAGE → CHANNEL_LEAVE
// ----------------------------------------------------------------------------

#[test]
fn test_full_join_flow() {
    let user_hub       = UserHub::new();
    let channel_hub    = ChannelHub::new();
    let media_peer_hub = MediaPeerHub::new();
    let (tx, _rx)      = mpsc::channel(10);

    // IDENTIFY
    user_hub.register("user_1", tx);
    assert!(user_hub.get("user_1").is_some());

    // CHANNEL_CREATE
    channel_hub.create("CH_1", config::MAX_PEERS_PER_CHANNEL);
    assert!(channel_hub.get("CH_1").is_some());

    // CHANNEL_JOIN
    let ch = channel_hub.get("CH_1").unwrap();
    assert!(ch.add_member("user_1").is_ok());
    media_peer_hub.insert(100, "user_1", "CH_1");

    assert!(ch.get_members().contains("user_1"));
    assert!(media_peer_hub.get(100).is_some());
    assert_eq!(media_peer_hub.get_channel_peers("CH_1").len(), 1);

    // CHANNEL_LEAVE / WS 종료
    ch.remove_member("user_1");
    media_peer_hub.remove(100);
    user_hub.unregister("user_1");

    assert!(!ch.get_members().contains("user_1"));
    assert!(media_peer_hub.get(100).is_none());
    assert!(user_hub.get("user_1").is_none());
}
