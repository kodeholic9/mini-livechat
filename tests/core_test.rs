use mini_livechat::core::{LiveChannelHub, LivePeerHub};
use mini_livechat::config;

#[test]
fn test_peer_join_and_channel_creation() {
    let channel_hub = LiveChannelHub::new();
    let peer_hub = LivePeerHub::new();

    let peer = peer_hub.join_channel("user_1", 100, "CH_1", &channel_hub).unwrap();

    assert_eq!(peer.member_id, "user_1");
    assert_eq!(peer.ssrc, 100);
    assert_eq!(peer.live_channel.channel_id, "CH_1");

    let channel = channel_hub.get_or_create("CH_1");
    let active_peers = channel.active_peers.read().unwrap();
    assert!(active_peers.contains_key(&100), "채널에 유저가 등록되어야 합니다.");
}

#[test]
fn test_memory_leak_prevention_with_weak_ref() {
    let channel_hub = LiveChannelHub::new();
    let peer_hub = LivePeerHub::new();

    let peer = peer_hub.join_channel("user_2", 200, "CH_2", &channel_hub).unwrap();
    let channel = channel_hub.get_or_create("CH_2");

    {
        let active_peers = channel.active_peers.read().unwrap();
        let weak_peer = active_peers.get(&200).unwrap();
        assert!(weak_peer.upgrade().is_some(), "유저가 살아있으므로 upgrade가 성공해야 합니다.");
    }

    peer_hub.peers.write().unwrap().remove(&200);
    drop(peer); 

    let active_peers = channel.active_peers.read().unwrap();
    let weak_peer = active_peers.get(&200).unwrap();
    
    assert!(
        weak_peer.upgrade().is_none(),
        "순환 참조(메모리 누수)가 발생했습니다! Weak 포인터가 여전히 살아있습니다."
    );
}

#[test]
fn test_channel_capacity_limit() {
    let channel_hub = LiveChannelHub::new();
    let peer_hub = LivePeerHub::new();
    let target_channel = "CH_FULL";

    for i in 0..config::MAX_PEERS_PER_CHANNEL {
        let ssrc = i as u32;
        let member_id = format!("user_{}", i);
        let result = peer_hub.join_channel(&member_id, ssrc, target_channel, &channel_hub);
        assert!(result.is_ok(), "최대 인원 도달 전에는 입장이 성공해야 합니다.");
    }

    let over_capacity_ssrc = 9999;
    let result = peer_hub.join_channel("user_overflow", over_capacity_ssrc, target_channel, &channel_hub);

    match result {
        Ok(_) => panic!("정원이 초과되었으므로 입장이 거절되어야 합니다."),
        Err(e) => {
            let err_msg = e.to_string();
            assert!(err_msg.contains("Capacity exceeded"), "기대했던 에러 메시지가 아닙니다.");
        }
    }
}