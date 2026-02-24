# Changelog

All notable changes to this project will be documented in this file.

---

## [TODO] DTLS-SRTP 키 도출 — 다음 세션 시작점

**조사 완료 내용** (2026-02-24)

**정규 파이프라인 확인:**
```
dtls::state::State  →  KeyingMaterialExporter 트레이트 구현
         ↓
webrtc-srtp::config::Config::extract_session_keys_from_dtls(state, is_client)
         ↓
srtp::context::Context::new(local_key, local_salt, profile, ...)
```
- `webrtc-rs/rtc/rtc-srtp/src/config.rs` 에 `extract_session_keys_from_dtls()` 공식 API 존재 확인 ✅
- `dtls::state::State` 가 `KeyingMaterialExporter` 트레이트 구현 확인 ✅
- **문제**: 현재 `do_handshake()` 는 `DTLSConn` 만 들고 있고, `DTLSConn.state` 가 `pub(crate)` 라 외부 접근 불가

**확정된 전략: `dtls` 버전 업그레이드 (포크 없음)**
- `webrtc-dtls 0.6.x` 이상부터 `export_keying_material()` 공개 API 추가 여부 확인 필요
- 포크/아키텍처 전환 없이 `Cargo.toml` 버전만 올리는 정규 방법

**⚠️ 롤백 완료 (2026-02-24)**
- `rtc-dtls/src/conn/mod.rs` 에 잘못 추가한 `export_keying_material()` 패치 롤백 ✅
- `[patch.crates-io]` 설정 추가 없음 (미진행) ✅

**다음 세션 작업 순서**:

  STEP 1. `webrtc-dtls` 최신 버전 API 확인
  - `webrtc-dtls 0.6.x` ~ `0.7.x` 의 `DTLSConn` pub fn 목록에서
    `export_keying_material()` 존재 여부 확인
  - 확인 방법: `cargo doc` 또는 crates.io 문서

  STEP 2. `Cargo.toml` 버전 업
  - `dtls = "0.17.1"` → 확인된 버전으로 변경
  - `webrtc-srtp` 도 동일 계열 버전으로 맞춤

  STEP 3. `do_handshake()` TODO 블록 실제 구현
  - `dtls_conn.export_keying_material(SRTP_MASTER_KEY_LABEL, &[], KEY_MATERIAL_LEN)`
  - `webrtc_srtp::config::Config::extract_session_keys_from_dtls()` 호출
  - `srtp::context::Context::new()` 로 컨텍스트 생성 → endpoint 에 설치

  STEP 4. `cargo build` 확인 (부장님이 직접 실행 후 에러 붙여넣기)
  ```bash
  cd C:\work\github\mini-livechat && cargo build 2>&1
  ```

  STEP 5. 빌드 성공 후 CHANGELOG [0.8.0] 작성

---

## [0.7.0] - 2026-02-24

### Phase 2 완료 — DTLS 핸드셰이크 연결 및 빌드 수정

#### media/dtls.rs
- `DtlsSessionMap` 추가: `SocketAddr → DtlsPacketTx` 맵, 핸드셰이크 중인 세션 패킷 라우팅
- `UdpConnAdapter` 재설계: `new()` → `(어댑터, tx)` 쌍 반환, 외부에서 패킷 주입 가능
- `start_dtls_handshake()` 시그니처 변경: `session_map` 파라미터 추가, 세션 등록/해제 자동 관리
- `ServerCert::generate()` 정리: 미사용 변수(`key_pem`) 제거, rcgen 0.14 API 정합
- `sha256_fingerprint()` private 유틸 함수로 정리

#### media/net.rs
- `run_udp_relay()` 시그니처 변경: `cert`, `session_map` 파라미터 추가
- `handle_dtls()` 실제 구현: 기존 세션 inject → 신규 세션 핸드셰이크 시작 분기
- `make_binding_response()` 버그 수정: `v4.ip().clone()` → `*v4.ip()`

#### media.rs
- `DtlsSessionMap` re-export 추가

#### lib.rs
- `ServerCert::generate()` 서버 시작 시 1회 생성, 실패 시 조기 종료
- `DtlsSessionMap` 생성 및 `run_udp_relay()` 에 전달
- DTLS fingerprint 시작 로그 추가

---

## [0.6.0] - 2026-02-24

### Phase 2 시작 — ICE Lite + DTLS-SRTP 기반 구조 재설계

#### core.rs
- `MediaPeer` → `Endpoint` 리네임 (`MediaPeer`는 호환성 alias 유지)
- `MediaPeerHub` 키 재설계: `by_ssrc` 제거 → `by_ufrag`(주키) + `by_addr`(핵패스 캐시)
- `TrackKind` enum 추가 (Audio / Video / Data)
- `Track` 구조체 추가 (ssrc + 종류) — BUNDLE 환경에서 ssrc는 라우팅 키가 아니라 Endpoint 내 메타데이터
- `latch()` 메서드 추가: STUN 콜드패스 후 by_addr 콜드패스 갱신

#### media/net.rs
- UDP 패킷 타입 판별 로직 추가 (STUN / DTLS / SRTP)
- STUN 핵들러: USERNAME ufrag 파싱 → latch → Binding Response
- DTLS 핵들러: Phase 2 스탈 (by_addr 조회만)
- SRTP 핵들러: by_addr O(1) 핫패스 조회 → 복호화 → 릴레이

#### protocol
- `ChannelJoinPayload`에 `ufrag` 필드 추가
- `Session`에 `current_ufrag` 필드 추가
- `collect_members()` 리팬터: by_ssrc 역조회 → Endpoint.tracks 기반

---

## [0.5.0] - 2026-02-23

### Added
- `src/http.rs` — HTTP REST API 핸들러 추가
  - `GET /channels` — 채널 목록 (id, member_count, capacity, created_at)
  - `GET /channels/{id}` — 채널 상세 + 현재 peer 목록 (user_id, ssrc)
- `src/lib.rs` — WS 라우터 + HTTP 라우터 merge 구조로 도입

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
