// author: kodeholic (powered by Claude)
// HTTP REST API 모듈

pub mod admin;
pub mod channel;
pub mod dto;
pub mod state;
pub mod trace;

// re-export: 기존 `use crate::http::*` 코드가 그대로 동작하도록
pub use state::HttpState;

// 핸들러 re-export (lib.rs 라우터 등록용)
pub use channel::{list_channels, get_channel};
pub use admin::{
    admin_status, admin_list_users, admin_get_user,
    admin_list_channels, admin_get_channel,
    admin_list_peers, admin_get_peer,
    admin_floor_revoke,
};
pub use trace::trace_stream;
