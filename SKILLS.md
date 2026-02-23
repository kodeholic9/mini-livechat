# Mini LiveChat — 개발 스킬 노트

이 문서는 본 프로젝트를 유지보수하거나 확장할 때 참고할 핵심 패턴과 결정 배경을 기록합니다.

---

## 1. 프로젝트 구조

```
src/
├── main.rs
├── lib.rs              — run_server(), 모듈 선언
├── config.rs           — 전역 상수 (포트, 타임아웃, 정원 등)
├── error.rs            — LiveError enum, LiveResult<T>
├── core.rs             — 상태 관리 3계층 (UserHub, ChannelHub, MediaPeerHub)
├── utils.rs            — current_timestamp()
├── net.rs              — UDP 미디어 릴레이 (구현 예정)
├── crypto.rs           — SRTP 암복호화 (구현 예정)
│
├── protocol.rs         — 서브모듈 선언 (mod.rs 대신 사용)
└── protocol/
    ├── opcode.rs       — client / server opcode 상수
    ├── error_code.rs   — u16 에러 코드 + LiveError 매핑
    ├── message.rs      — GatewayPacket + payload 타입들
    └── protocol.rs     — AppState, ws_handler, op 핸들러들
```

---

## 2. 상태 관리 계층

### UserHub
- IDENTIFY 수신 시 등록, WS 종료 시 제거
- `user_id → Arc<User>` 매핑
- `User.tx: BroadcastTx` — 브로드캐스트 라우팅 테이블
- `User.last_seen: AtomicU64` — 좀비 세션 감지용
- `broadcast_to(user_ids, packet_json, exclude)` — 선택적 브로드캐스트

### ChannelHub
- `channel_id → Arc<Channel>` 매핑
- `Channel.members: RwLock<HashSet<user_id>>` — 채널 내 멤버 목록
- `Channel.capacity` — 정원 제한
- `Channel.created_at` — 확장성 고려 타임스탬프

### MediaPeerHub
- CHANNEL_JOIN 시 등록, CHANNEL_LEAVE / WS 종료 시 제거
- `ssrc → Arc<MediaPeer>` 매핑 — O(1) 조회
- `MediaPeer.user_id` — ssrc → user_id 역매핑
- `MediaPeer.channel_id` — 릴레이 대상 채널
- `MediaPeer.address` — UDP 패킷 출발지 주소 (Symmetric RTP Latching)
- `MediaPeer.last_seen` — 좀비 피어 감지용
- `get_channel_peers(channel_id)` — 미디어 릴레이 대상 목록

---

## 3. 브로드캐스트 경로

```
핸들러
    → ChannelHub.get(channel_id).get_members()   // user_id HashSet
    → UserHub.broadcast_to(members, json, exclude)
        → users.read() 로 tx 일괄 수집
        → tx.send(json) 비동기 전송
```

### WS 송수신 분리 패턴
```rust
let (mut ws_tx, mut ws_rx) = socket.split();
let (broadcast_tx, mut broadcast_rx) = mpsc::channel(EGRESS_QUEUE_SIZE);

// rx_loop: broadcast_rx → ws_tx (별도 태스크)
tokio::spawn(async move {
    while let Some(json) = broadcast_rx.recv().await {
        ws_tx.send(Message::Text(json)).await;
    }
});

// tx_loop: ws_rx → 핸들러 → broadcast_tx
while let Some(msg) = ws_rx.next().await { ... }
```

---

## 4. 에러 처리 패턴

```rust
// 핸들러에서 에러 발생 시
return send(tx, error_packet(LiveError::ChannelFull(channel_id))).await;

// error_packet 내부
fn error_packet(err: LiveError) -> String {
    make_packet(server::ERROR, ErrorPayload {
        code:   to_error_code(&err),   // error_code.rs에서 u16 변환
        reason: err.to_string(),
    })
}
```

---

## 5. 미디어 릴레이 핫패스 (net.rs 구현 시 참고)

```
UDP 패킷 수신
    → RTP 헤더에서 ssrc 파싱
    → MediaPeerHub.get(ssrc)           // O(1)
    → peer.update_address(src_addr)    // Symmetric RTP Latching
    → peer.touch()                     // last_seen 갱신
    → SRTP 복호화 (inbound_srtp)
    → MediaPeerHub.get_channel_peers(peer.channel_id)
    → 각 peer.address로 SRTP 재암호화 후 UDP 전송
```

---

## 6. 좀비 감지 (구현 예정)

```rust
// 별도 tokio 태스크로 주기적 실행
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;

        // WS 세션 좀비
        for user_id in user_hub.find_zombies(ZOMBIE_TIMEOUT_MS) {
            // cleanup 처리
        }

        // 미디어 피어 좀비
        for ssrc in media_peer_hub.find_zombies(ZOMBIE_TIMEOUT_MS) {
            media_peer_hub.remove(ssrc);
        }
    }
});
```

---

## 7. 테스트 전략

### 유닛 테스트 (`tests/core_test.rs`)
- 자료구조 레벨 검증 (허브 등록/해제, 정원 초과, 중복 입장, 좀비 감지)
- 동기 테스트로 빠른 피드백

### 통합 테스트 (`tests/integration_test.rs`)
- 랜덤 포트로 실제 서버 기동 (`portpicker`)
- `tokio-tungstenite` WS 클라이언트로 실제 JSON 패킷 주고받기
- 브로드캐스트 검증은 클라이언트 2개로 수행
- 공통 헬퍼: `identify()`, `join_channel()` 로 시나리오 간결화

---

## 8. 주요 의존성

| 크레이트 | 용도 |
|---|---|
| tokio | 비동기 런타임 |
| axum | HTTP/WebSocket 서버 |
| serde / serde_json | 패킷 직렬화 |
| futures-util | WS split, stream/sink |
| tracing | 구조화 로깅 |
| tokio-tungstenite | 테스트용 WS 클라이언트 |
| portpicker | 테스트용 랜덤 포트 |
