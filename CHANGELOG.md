# Changelog

All notable changes to this project will be documented in this file.

---

## [0.4.0] - 2026-02-23

### Added
- `src/config.rs` — `DEFAULT_SECRET_KEY` 상수 추가
- `src/protocol/protocol.rs` — IDENTIFY 토큰 검증 구현
  - 환경변수 `LIVECHAT_SECRET` 우선, 없으면 `DEFAULT_SECRET_KEY` 폴백
  - 불일치 시 `InvalidToken` (1001) 에러 반환

### Changed
- `tests/integration_test.rs` — `TEST_SECRET` 상수 추가, 모든 IDENTIFY 토큰을 환경변수와 동기화

---

## [0.3.0] - 2026-02-23

### Added
- `lib.rs` — `run_zombie_reaper()` 백그라운드 태스크 추가
  - `HEARTBEAT_INTERVAL_MS` 주기로 순회
  - heartbeat 없는 좀비 WS 세션 자동 제거 (`UserHub`)
  - UDP 패킷 없는 좀비 MediaPeer 자동 제거 (`MediaPeerHub`)

### Changed
- `src/media/srtp.rs` — Phase 1 평문 패스스루로 확정, TODO 제거 및 Phase 2 계획 명시
- `src/core.rs` — `SrtpContext {}` 직접 생성 → `SrtpContext::new()` 으로 통일
- `Cargo.toml` — `webrtc-srtp`, `rcgen` 제거 (Phase 2에서 재추가 예정)
- `lib.rs` — 허브 인스턴스를 `run_server()` 상단에서 생성 후 `Arc::clone` 으로 공유

---

## [0.2.0] - 2026-02-23

### Added

#### 프로토콜 레이어 (`src/protocol/`)
- `opcode.rs` — client/server 네임스페이스로 분리된 opcode 상수 정의
- `error_code.rs` — u16 에러 코드 상수 + `LiveError` → 에러 코드 변환 함수
- `message.rs` — `GatewayPacket` 봉투 구조체 및 각 op별 payload 타입 정의
- `protocol.rs` — `AppState`, `ws_handler`, 개별 op 핸들러 구현

#### 시그널링 프로토콜
- 디스코드 스타일 opcode 기반 패킷 구조 채택 `{ "op": N, "d": { ... } }`
- HELLO / HEARTBEAT / HEARTBEAT_ACK / IDENTIFY / READY 흐름 구현
- CHANNEL_CREATE / CHANNEL_JOIN / CHANNEL_LEAVE / CHANNEL_UPDATE / CHANNEL_DELETE 핸들러
- MESSAGE_CREATE — 채널 내 전원 브로드캐스트
- ERROR 응답 (op: 201) — 에러 코드 + reason 포함

#### 브로드캐스트 아키텍처
- WS `split()` 으로 송수신 분리, `tokio::mpsc` 기반 내부 브로드캐스트 버스 구성
- `UserHub.broadcast_to()` — user_id 목록 기반 선택적 브로드캐스트
- 발신자 제외(exclude) 옵션 지원

#### 상태 관리 (`src/core.rs`) — 전면 재설계
- `UserHub` — IDENTIFY 시 등록되는 전역 라우팅 테이블, `User(tx, last_seen)`
- `ChannelHub` — 채널 정의 및 멤버 목록 관리, `Channel(channel_id, capacity, created_at, members)`
- `MediaPeerHub` — 미디어 릴레이 핫패스 전용 O(1) 조회, `MediaPeer(ssrc, user_id, channel_id, address, last_seen, srtp)`
- 좀비 세션/피어 감지 — `find_zombies(timeout_ms)` 메서드

#### 에러 처리 (`src/error.rs`)
- `LiveError` enum 전면 재설계 (1xxx 인증, 2xxx 채널, 3xxx 메시지, 9xxx 서버 내부)

#### 테스트
- `tests/core_test.rs` — UserHub, ChannelHub, MediaPeerHub 유닛 테스트 (15개)
- `tests/integration_test.rs` — 실제 서버 기동 후 WS 클라이언트 시나리오 테스트 (7개)

### Changed
- `src/config.rs` — `HEARTBEAT_INTERVAL_MS`, `MAX_MESSAGE_LENGTH` 상수 추가
- `src/lib.rs` — `mod signaling` 제거, `mod protocol` 교체, `AppState` 허브 구조 반영
- `Cargo.toml` — `[dev-dependencies]` 추가: `tokio-tungstenite`, `portpicker`

### Removed
- `src/signaling.rs` — `src/protocol/` 로 대체 (역할 종료)
- 기존 `LiveChannelHub`, `LiveChannel`, `LivePeerHub`, `LivePeer` 구조체 제거
  → `ChannelHub`/`Channel`, `MediaPeerHub`/`MediaPeer` 로 재설계

---

## [0.1.0] - 2026-02-22

### Added
- 초기 프로젝트 구조 설계
- `LivePeerHub`, `LiveChannelHub` — Arc/Weak 기반 메모리 안전 상태 관리
- WebSocket 시그널링 엔드포인트 `ws://localhost:8080/ws`
- WS 연결 종료 시 peer/channel 자동 클린업
- `config.rs` — 서버 상수 관리
- `error.rs` — `LiveError` enum, `LiveResult<T>`
- 유닛 테스트 3개 (peer join, 메모리 누수 방지, 채널 정원 제한)
