# Mini LiveChat — Claude 작업 컨텍스트

이 파일은 새 대화에서 Claude가 프로젝트를 빠르게 파악하고 이어서 작업하기 위한 참조 파일입니다.

---

## 프로젝트 개요

- **언어/프레임워크**: Rust + Tokio + Axum
- **목적**: 무전(PTT) 및 실시간 미디어 릴레이 백엔드 서버
- **경로**: `C:\work\github\mini-livechat`

---

## 현재 구현 상태

### 완료
- 디스코드 스타일 opcode 기반 WS 시그널링 프로토콜
- `UserHub` / `ChannelHub` / `MediaPeerHub` 3계층 상태 관리
- WS 송수신 분리 + mpsc 브로드캐스트 버스
- UDP 릴레이 루프 + Symmetric RTP Latching (`media/net.rs`)
- RTP 평문 패스스루 (`media/srtp.rs`) — Phase 1 확정
- 좀비 세션/피어 자동 종료 태스크 (`run_zombie_reaper`)
- IDENTIFY Secret Key 토큰 검증 (`LIVECHAT_SECRET` 환경변수)
- 유닛 테스트 15개, 통합 테스트 7개

### 미완료
1. SRTP 암복호화 Phase 2 (앱: pre-shared key, 브라우저: DTLS-SRTP) — 클라이언트 준비 후

---

## 소스 구조

```
src/
├── main.rs
├── lib.rs              — run_server(), mod 선언
├── config.rs           — 전역 상수
├── error.rs            — LiveError enum (1xxx~9xxx)
├── core.rs             — UserHub, ChannelHub, MediaPeerHub
├── utils.rs            — current_timestamp()
│
├── media.rs            — pub use net::run_udp_relay
└── media/
    ├── net.rs          — UDP 수신 루프, RTP 파싱, 릴레이
    └── srtp.rs         — SrtpContext (decrypt/encrypt)
│
├── protocol.rs         — 서브모듈 선언
└── protocol/
    ├── opcode.rs       — client/server opcode 상수
    ├── error_code.rs   — u16 에러 코드 + LiveError 매핑
    ├── message.rs      — GatewayPacket + payload 타입
    └── protocol.rs     — AppState, ws_handler, op 핸들러

tests/
├── core_test.rs        — 유닛 테스트 (15개)
└── integration_test.rs — WS 통합 테스트 (7개)
```

---

## 핵심 자료구조

```rust
// 전역 라우팅 테이블 (IDENTIFY 시 등록)
UserHub
    users: RwLock<HashMap<user_id, Arc<User>>>
        User { tx: BroadcastTx, last_seen: AtomicU64 }

// 채널 멤버 관리
ChannelHub
    channels: RwLock<HashMap<channel_id, Arc<Channel>>>
        Channel { channel_id, capacity, created_at, members: RwLock<HashSet<user_id>> }

// 미디어 핫패스 O(1) 조회
MediaPeerHub
    by_ssrc: RwLock<HashMap<ssrc, Arc<MediaPeer>>>
        MediaPeer { ssrc, user_id, channel_id, address, last_seen,
                    inbound_srtp: Mutex<SrtpContext>,
                    outbound_srtp: Mutex<SrtpContext> }
```

---

## 브로드캐스트 경로

```
핸들러
    → ChannelHub.get(channel_id).get_members()     // HashSet<user_id>
    → UserHub.broadcast_to(members, json, exclude) // tx.send()
```

## UDP 릴레이 경로

```
recv_from()
    → parse_ssrc() (offset 8, big-endian)
    → MediaPeerHub.get(ssrc)
    → peer.update_address(src_addr)        // Symmetric RTP Latching
    → peer.inbound_srtp.decrypt(packet)    // TODO: 실제 구현
    → get_channel_peers(channel_id) 순회
    → target.outbound_srtp.encrypt()       // TODO: 실제 구현
    → socket.send_to(encrypted, addr)
```

---

## 코딩 규칙

- 파일 상단 `// author: kodeholic (powered by Claude)` 명시
- 매직 넘버 금지 → `config.rs` 상수 사용
- `unwrap()` 남용 금지 → `LiveResult<T>` 또는 로그 후 `continue`
- 새 기능 추가 시 `CHANGELOG.md` 업데이트

---

## 자주 쓰는 명령

```bash
cargo build
cargo test
cargo test --test core_test
cargo test --test integration_test
RUST_LOG=trace cargo run
```
