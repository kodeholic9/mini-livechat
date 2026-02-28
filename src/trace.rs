// author: kodeholic (powered by Claude)
//
// TraceHub — 시그널링 이벤트 실시간 관찰 버스
//
// 구조:
//   핸들러(protocol.rs, floor.rs)
//       └── TraceHub::publish(event)
//               └── broadcast::Sender  (채널 구독자 수에 무관하게 O(1) publish)
//                       ├── SSE subscriber (lctrace 프로세스 1)
//                       └── SSE subscriber (lctrace 프로세스 2)
//
// Vue EventBus 패턴과 동일:
//   emit(event)  ≈ publish()
//   $on(handler) ≈ subscribe()
//
// 구독자가 없을 때 publish는 그냥 drop (서버 성능에 무영향)
// 구독자가 느리면 lagged 에러 반환 — 구독자 쪽에서 처리

use std::sync::Arc;
use tokio::sync::broadcast;
use serde::Serialize;
use crate::utils::current_timestamp;

/// 브로드캐스트 채널 버퍼 크기
/// 구독자가 느릴 때 최대 보유 이벤트 수 — 초과 시 오래된 이벤트 drop
const TRACE_BUF: usize = 512;

// ----------------------------------------------------------------------------
// [TraceEvent] — 관찰 가능한 시그널링 이벤트
// ----------------------------------------------------------------------------

/// 이벤트 방향
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TraceDir {
    /// 클라이언트 → 서버 (C→S)
    In,
    /// 서버 → 클라이언트 (S→C)
    Out,
    /// 서버 내부 (시스템)
    Sys,
}

/// 하나의 시그널링 이벤트
#[derive(Debug, Clone, Serialize)]
pub struct TraceEvent {
    /// Unix millis
    pub ts:         u64,
    /// 방향 (in / out / sys)
    pub dir:        TraceDir,
    /// 채널 ID (없으면 None — IDENTIFY 등)
    pub channel_id: Option<String>,
    /// 관련 user_id
    pub user_id:    Option<String>,
    /// opcode 번호
    pub op:         u8,
    /// opcode 이름 (예: "FLOOR_REQUEST")
    pub op_name:    String,
    /// 이벤트 요약 메시지
    pub summary:    String,
}

impl TraceEvent {
    pub fn new(
        dir:        TraceDir,
        channel_id: Option<&str>,
        user_id:    Option<&str>,
        op:         u8,
        op_name:    &str,
        summary:    impl Into<String>,
    ) -> Self {
        Self {
            ts:         current_timestamp(),
            dir,
            channel_id: channel_id.map(str::to_string),
            user_id:    user_id.map(str::to_string),
            op,
            op_name:    op_name.to_string(),
            summary:    summary.into(),
        }
    }
}

// ----------------------------------------------------------------------------
// [TraceHub]
// ----------------------------------------------------------------------------

pub struct TraceHub {
    tx: broadcast::Sender<TraceEvent>,
}

impl TraceHub {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(TRACE_BUF);
        Arc::new(Self { tx })
    }

    /// 이벤트 publish — 구독자가 없으면 조용히 무시
    pub fn publish(&self, event: TraceEvent) {
        // send 실패(구독자 없음)는 정상 케이스 — 무시
        let _ = self.tx.send(event);
    }

    /// SSE 구독자 생성 — 각 HTTP 연결마다 호출
    pub fn subscribe(&self) -> broadcast::Receiver<TraceEvent> {
        self.tx.subscribe()
    }
}
