// author: kodeholic (powered by Claude)
// 네트워크 로직과 철저히 분리된, 순수 비즈니스 상태 관리 모듈입니다.

pub mod channel;
pub mod floor;
pub mod media_peer;
pub mod user;

// re-export: 기존 `use crate::core::*` 코드가 그대로 동작하도록
pub use user::{BroadcastTx, User, UserHub};
pub use channel::{Channel, ChannelHub};
pub use floor::{FloorControl, FloorControlState, FloorIndicator, FloorQueueEntry};
pub use media_peer::{Endpoint, MediaPeer, MediaPeerHub, Track, TrackKind};
