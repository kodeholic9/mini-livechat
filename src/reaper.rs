// author: kodeholic (powered by Claude)
// 좀비 세션 자동 종료 태스크
//
// 주기마다 수행:
//   1. 좀비 User (WS 하트비트 없음) 제거 + 소속 채널 멤버에서 제외
//   2. 좀비 Endpoint (UDP 패킷 없음) 제거
//   3. 단절된 DTLS 핸드셰이크 세션 제거 (tx 닫힌 세션 정리)
//   4. Floor 타임아웃 체크 (ping_timeout / max_duration Revoke)

use std::sync::Arc;
use tracing::info;

use crate::config;
use crate::core::{ChannelHub, MediaPeerHub, UserHub};
use crate::media::DtlsSessionMap;
use crate::trace::TraceHub;

pub async fn run_zombie_reaper(
    user_hub:     Arc<UserHub>,
    channel_hub:  Arc<ChannelHub>,
    media_hub:    Arc<MediaPeerHub>,
    session_map:  Arc<DtlsSessionMap>,
    trace_hub:    Arc<TraceHub>,
) {
    let interval  = tokio::time::Duration::from_millis(config::REAPER_INTERVAL_MS);
    let mut timer = tokio::time::interval(interval);
    timer.tick().await; // 첫 틱 skip (startup 시 즉시 실행 방지)

    info!("[zombie-reaper] Started (interval={}ms, timeout={}ms)",
        config::REAPER_INTERVAL_MS, config::ZOMBIE_TIMEOUT_MS);

    loop {
        timer.tick().await;

        // 1. 좀비 User 정리
        //    WS 하트비트가 ZOMBIE_TIMEOUT_MS 동안 없으면 제거
        //    + 대상 유저가 소속된 모든 채널 멤버에서 제외
        let dead_users = user_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for uid in &dead_users {
            // 채널 멤버에서 먼저 제거 (채널 유지)
            let channels = channel_hub.channels.read().unwrap();
            for ch in channels.values() {
                ch.remove_member(uid);
            }
            drop(channels);
            user_hub.unregister(uid);
            info!("[zombie-reaper] user={} removed (no heartbeat)", uid);
        }

        // 2. 좀비 Endpoint 정리
        //    UDP 패킷이 ZOMBIE_TIMEOUT_MS 동안 없으면 제거
        let dead_peers = media_hub.find_zombies(config::ZOMBIE_TIMEOUT_MS);
        for ufrag in &dead_peers {
            media_hub.remove(ufrag);
            info!("[zombie-reaper] peer ufrag={} removed (no media)", ufrag);
        }

        // 3. 단절된 DTLS 세션 정리
        //    tx가 닫힌 세션 = 핸드셰이크 태스크가 종료됐거나 타임아웃
        let stale = session_map.remove_stale().await;
        for addr in &stale {
            info!("[zombie-reaper] dtls session stale addr={}", addr);
        }

        // 4. Floor 타임아웃 체크 (ping_timeout / max_duration Revoke)
        crate::protocol::floor::check_floor_timeouts(&user_hub, &channel_hub, Some(&trace_hub)).await;

        let total = dead_users.len() + dead_peers.len() + stale.len();
        if total > 0 {
            info!("[zombie-reaper] Cleaned {} user(s), {} peer(s), {} dtls session(s)",
                dead_users.len(), dead_peers.len(), stale.len());
        }
    }
}
