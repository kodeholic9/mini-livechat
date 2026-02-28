// author: kodeholic (powered by Claude)
// FloorControl — MBCP TS 24.380 기반 Floor Control 상태 관리
// Channel당 1개 인스턴스, Channel.floor(Mutex)로 보호

use std::collections::VecDeque;

use crate::config;
use crate::utils::current_timestamp;

/// Floor 표시자 — 발언의 성격/우선순위를 나타냄 (MBCP Floor Indicator)
#[derive(Debug, Clone, PartialEq)]
pub enum FloorIndicator {
    Normal,        // 일반 발언
    Broadcast,     // 단방향 방송 (청취자 응답 없음)
    ImminentPeril, // 임박한 위험 — 일반보다 높은 우선순위
    Emergency,     // 긴급 — 최고 우선순위, priority 무관 즉시 Preempt
}

/// Floor Control 서버 상태머신 (MBCP G: 상태)
#[derive(Debug, Clone, PartialEq)]
pub enum FloorControlState {
    Idle,  // G: Floor Idle  — 발언권 없음
    Taken, // G: Floor Taken — 발언권 점유 중
}

/// 대기열 항목 — Floor Request가 Deny 대신 Queue에 들어올 때
#[derive(Debug, Clone)]
pub struct FloorQueueEntry {
    pub user_id:   String,
    pub priority:  u8,
    pub indicator: FloorIndicator,
    pub queued_at: u64,
}

/// 채널별 Floor Control 상태 (Mutex<FloorControl>로 보호)
pub struct FloorControl {
    /// 현재 서버 상태 (G: Floor Idle / G: Floor Taken)
    pub state:           FloorControlState,
    /// 현재 발언 중인 user_id (MBCP: Granted Party's Identity)
    pub floor_taken_by:  Option<String>,
    /// 발언권 획득 시각 — FLOOR_MAX_TAKEN_MS 초과 시 Revoke
    pub floor_taken_at:  Option<u64>,
    /// 현재 holder의 우선순위 — Preemption 판단 기준
    pub floor_priority:  u8,
    /// 현재 발언의 성격 (Emergency 여부 등)
    pub floor_indicator: FloorIndicator,
    /// 발언 대기열 — priority 내림차순, 동일 priority는 FIFO
    pub queue:           VecDeque<FloorQueueEntry>,
    /// 마지막 클라이언트 Ping 수신 시각 — 타임아웃 감지용
    pub last_ping_at:    u64,
}

impl FloorControl {
    pub fn new() -> Self {
        Self {
            state:           FloorControlState::Idle,
            floor_taken_by:  None,
            floor_taken_at:  None,
            floor_priority:  0,
            floor_indicator: FloorIndicator::Normal,
            queue:           VecDeque::new(),
            last_ping_at:    0,
        }
    }

    /// 발언권 상태 초기화 (Release/Revoke 후 공통 처리)
    pub fn clear_taken(&mut self) {
        self.state           = FloorControlState::Idle;
        self.floor_taken_by  = None;
        self.floor_taken_at  = None;
        self.floor_priority  = 0;
        self.floor_indicator = FloorIndicator::Normal;
        self.last_ping_at    = 0;
    }

    /// 발언권 부여 (Grant)
    pub fn grant(&mut self, user_id: String, priority: u8, indicator: FloorIndicator) {
        self.state           = FloorControlState::Taken;
        self.floor_taken_by  = Some(user_id);
        self.floor_taken_at  = Some(current_timestamp());
        self.floor_priority  = priority;
        self.floor_indicator = indicator;
        self.last_ping_at    = current_timestamp(); // Grant 시점을 초기값으로 설정
    }

    /// 대기열에 요청 추가 — priority 내림차순 삽입 (높은 priority가 앞)
    /// 같은 user_id가 이미 있으면 갱신
    pub fn enqueue(&mut self, user_id: String, priority: u8, indicator: FloorIndicator) {
        self.queue.retain(|e| e.user_id != user_id);
        let entry = FloorQueueEntry { user_id, priority, indicator, queued_at: current_timestamp() };
        let pos = self.queue.iter().position(|e| e.priority < entry.priority)
            .unwrap_or(self.queue.len());
        self.queue.insert(pos, entry);
    }

    /// 대기열에서 다음 후보 꺼내기
    pub fn dequeue_next(&mut self) -> Option<FloorQueueEntry> {
        self.queue.pop_front()
    }

    /// 대기열에서 특정 user_id 제거 (CHANNEL_LEAVE 등)
    pub fn remove_from_queue(&mut self, user_id: &str) {
        self.queue.retain(|e| e.user_id != user_id);
    }

    /// 대기열 내 user_id의 순서 반환 (1-based, 없으면 None)
    pub fn queue_position(&self, user_id: &str) -> Option<usize> {
        self.queue.iter().position(|e| e.user_id == user_id).map(|i| i + 1)
    }

    /// Preemption 가능 여부 판단
    /// Emergency는 priority 무관 항상 true
    /// 그 외는 요청자 priority > 현재 holder priority 일 때만 true
    pub fn can_preempt(&self, req_priority: u8, req_indicator: &FloorIndicator) -> bool {
        if self.state != FloorControlState::Taken { return false; }
        match req_indicator {
            FloorIndicator::Emergency => true,
            _ => req_priority > self.floor_priority,
        }
    }

    /// 클라이언트 Ping 수신 — last_ping_at 갱신
    pub fn on_ping(&mut self) {
        self.last_ping_at = current_timestamp();
    }

    /// Ping 타임아웃 여부 (last_ping_at 기준)
    pub fn is_ping_timeout(&self) -> bool {
        if self.state != FloorControlState::Taken { return false; }
        current_timestamp().saturating_sub(self.last_ping_at) >= config::FLOOR_PING_TIMEOUT_MS
    }

    /// 최대 발언 시간 초과 여부
    pub fn is_max_taken_exceeded(&self) -> bool {
        if let Some(taken_at) = self.floor_taken_at {
            current_timestamp().saturating_sub(taken_at) >= config::FLOOR_MAX_TAKEN_MS
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_floor_is_idle() {
        let f = FloorControl::new();
        assert_eq!(f.state, FloorControlState::Idle);
        assert!(f.floor_taken_by.is_none());
        assert!(f.queue.is_empty());
    }

    #[test]
    fn grant_transitions_to_taken() {
        let mut f = FloorControl::new();
        f.grant("alice".into(), 100, FloorIndicator::Normal);
        assert_eq!(f.state, FloorControlState::Taken);
        assert_eq!(f.floor_taken_by.as_deref(), Some("alice"));
        assert_eq!(f.floor_priority, 100);
        assert!(f.floor_taken_at.is_some());
    }

    #[test]
    fn clear_taken_resets_to_idle() {
        let mut f = FloorControl::new();
        f.grant("alice".into(), 100, FloorIndicator::Normal);
        f.clear_taken();
        assert_eq!(f.state, FloorControlState::Idle);
        assert!(f.floor_taken_by.is_none());
        assert!(f.floor_taken_at.is_none());
        assert_eq!(f.floor_priority, 0);
    }

    #[test]
    fn enqueue_priority_ordering() {
        let mut f = FloorControl::new();
        f.enqueue("low".into(), 50, FloorIndicator::Normal);
        f.enqueue("high".into(), 200, FloorIndicator::Normal);
        f.enqueue("mid".into(), 100, FloorIndicator::Normal);
        // 높은 priority가 앞에
        let next = f.dequeue_next().unwrap();
        assert_eq!(next.user_id, "high");
        let next = f.dequeue_next().unwrap();
        assert_eq!(next.user_id, "mid");
        let next = f.dequeue_next().unwrap();
        assert_eq!(next.user_id, "low");
        assert!(f.dequeue_next().is_none());
    }

    #[test]
    fn enqueue_same_user_updates() {
        let mut f = FloorControl::new();
        f.enqueue("alice".into(), 50, FloorIndicator::Normal);
        f.enqueue("alice".into(), 200, FloorIndicator::Emergency);
        assert_eq!(f.queue.len(), 1);
        assert_eq!(f.queue[0].priority, 200);
    }

    #[test]
    fn remove_from_queue() {
        let mut f = FloorControl::new();
        f.enqueue("a".into(), 100, FloorIndicator::Normal);
        f.enqueue("b".into(), 100, FloorIndicator::Normal);
        f.remove_from_queue("a");
        assert_eq!(f.queue.len(), 1);
        assert_eq!(f.queue[0].user_id, "b");
    }

    #[test]
    fn queue_position_1based() {
        let mut f = FloorControl::new();
        f.enqueue("a".into(), 200, FloorIndicator::Normal);
        f.enqueue("b".into(), 100, FloorIndicator::Normal);
        assert_eq!(f.queue_position("a"), Some(1));
        assert_eq!(f.queue_position("b"), Some(2));
        assert_eq!(f.queue_position("c"), None);
    }

    #[test]
    fn can_preempt_emergency_always_true() {
        let mut f = FloorControl::new();
        f.grant("alice".into(), 255, FloorIndicator::Normal);
        // Emergency는 priority 무관 항상 preempt
        assert!(f.can_preempt(1, &FloorIndicator::Emergency));
    }

    #[test]
    fn can_preempt_higher_priority() {
        let mut f = FloorControl::new();
        f.grant("alice".into(), 100, FloorIndicator::Normal);
        assert!(f.can_preempt(200, &FloorIndicator::Normal));
        assert!(!f.can_preempt(100, &FloorIndicator::Normal));
        assert!(!f.can_preempt(50, &FloorIndicator::Normal));
    }

    #[test]
    fn can_preempt_idle_returns_false() {
        let f = FloorControl::new();
        assert!(!f.can_preempt(255, &FloorIndicator::Emergency));
    }

    #[test]
    fn on_ping_updates_last_ping_at() {
        let mut f = FloorControl::new();
        f.grant("alice".into(), 100, FloorIndicator::Normal);
        let t1 = f.last_ping_at;
        std::thread::sleep(std::time::Duration::from_millis(5));
        f.on_ping();
        assert!(f.last_ping_at >= t1);
    }

    #[test]
    fn is_ping_timeout_idle_returns_false() {
        let f = FloorControl::new();
        assert!(!f.is_ping_timeout());
    }

    #[test]
    fn is_max_taken_exceeded_idle_returns_false() {
        let f = FloorControl::new();
        assert!(!f.is_max_taken_exceeded());
    }

    #[test]
    fn is_max_taken_exceeded_fresh_grant_false() {
        let mut f = FloorControl::new();
        f.grant("alice".into(), 100, FloorIndicator::Normal);
        assert!(!f.is_max_taken_exceeded());
    }
}
