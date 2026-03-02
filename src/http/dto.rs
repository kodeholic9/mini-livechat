// author: kodeholic (powered by Claude)
// HTTP 응답 DTO — Admin / 일반 조회 공용

use serde::Serialize;

// ----------------------------------------------------------------------------
// [일반 조회]
// ----------------------------------------------------------------------------

/// GET /channels 응답 아이템
#[derive(Serialize)]
pub struct ChannelSummary {
    pub channel_id:    String,
    pub freq:          String,
    pub name:          String,
    pub mode:          String,
    pub member_count:  usize,
    pub capacity:      usize,
    pub created_at:    u64,
}

/// GET /channels/{id} 응답
#[derive(Serialize)]
pub struct ChannelDetail {
    pub channel_id:   String,
    pub freq:         String,
    pub name:         String,
    pub mode:         String,
    pub member_count: usize,
    pub capacity:     usize,
    pub created_at:   u64,
    pub peers:        Vec<PeerInfo>,
}

/// peer 정보 (채널 상세에 포함)
#[derive(Serialize)]
pub struct PeerInfo {
    pub user_id: String,
    pub ssrc:    u32,
}

// ----------------------------------------------------------------------------
// [Admin]
// ----------------------------------------------------------------------------

/// GET /admin/status
#[derive(Serialize)]
pub struct ServerStatus {
    pub uptime_secs:   u64,
    pub user_count:    usize,
    pub channel_count: usize,
    pub peer_count:    usize,
    pub floor_active:  usize, // Floor Taken 상태인 채널 수
}

/// GET /admin/users 아이템
#[derive(Serialize)]
pub struct AdminUserSummary {
    pub user_id:      String,
    pub priority:     u8,
    pub last_seen_ms: u64,   // Unix millis
    pub idle_secs:    u64,   // 마지막 heartbeat 이후 경과 초
}

/// GET /admin/users/{user_id}
#[derive(Serialize)]
pub struct AdminUserDetail {
    pub user_id:      String,
    pub priority:     u8,
    pub last_seen_ms: u64,
    pub idle_secs:    u64,
    pub channels:     Vec<String>, // 소속 채널 id 목록
}

/// GET /admin/channels 아이템 (Floor 상태 포함)
#[derive(Serialize)]
pub struct AdminChannelSummary {
    pub channel_id:    String,
    pub freq:          String,
    pub name:          String,
    pub mode:          String,
    pub member_count:  usize,
    pub capacity:      usize,
    pub floor_state:   String,          // "idle" | "taken"
    pub floor_holder:  Option<String>,  // Taken 시 holder user_id
    pub queue_len:     usize,
}

/// GET /admin/channels/{id}
#[derive(Serialize)]
pub struct AdminChannelDetail {
    pub channel_id:       String,
    pub freq:             String,
    pub name:             String,
    pub mode:             String,
    pub capacity:         usize,
    pub created_at:       u64,
    pub members:          Vec<String>,
    pub floor_state:      String,
    pub floor_holder:     Option<String>,
    pub floor_taken_secs: Option<u64>,  // Taken된 후 경과 초
    pub floor_priority:   u8,
    pub queue_len:        usize,
    pub queue:            Vec<AdminQueueEntry>,
    pub peers:            Vec<AdminPeerSummary>,
}

#[derive(Serialize)]
pub struct AdminQueueEntry {
    pub user_id:    String,
    pub priority:   u8,
    pub queued_at:  u64,
    pub wait_secs:  u64,
}

/// GET /admin/peers 아이템
#[derive(Serialize)]
pub struct AdminPeerSummary {
    pub ufrag:      String,
    pub user_id:    String,
    pub channel_id: String,
    pub address:    Option<String>,
    pub idle_secs:  u64,
    pub srtp_ready: bool,
}

/// GET /admin/peers/{ufrag}
#[derive(Serialize)]
pub struct AdminPeerDetail {
    pub ufrag:      String,
    pub user_id:    String,
    pub channel_id: String,
    pub address:    Option<String>,
    pub last_seen:  u64,
    pub idle_secs:  u64,
    pub srtp_ready: bool,
    pub tracks:     Vec<AdminTrack>,
}

#[derive(Serialize)]
pub struct AdminTrack {
    pub ssrc: u32,
    pub kind: String,
}
