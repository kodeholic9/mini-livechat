# Changelog

All notable changes to this project will be documented in this file.

---

## [TODO] (다음 세션)

### 즉시 해야 할 것 — `cargo build` 통과 확인

- [ ] `cargo build` 실행 후 컴파일 에러 없는지 확인
- [ ] warning 정리 (미사용 변수, dead_code 등)

### E2E 시나리오 테스트 (브라우저 2탭)

- [ ] 탭A IDENTIFY(priority=100) + JOIN → 탭B IDENTIFY(priority=100) + JOIN
- [ ] 탭A PTT 누름 → FLOOR_GRANTED 수신 확인 / 탭B FLOOR_TAKEN 수신 확인
- [ ] 탭A PTT 놓음 → 탭A FLOOR_IDLE 수신 / 탭B FLOOR_IDLE 수신
- [ ] 탭A 발언 중 탭B PTT → FLOOR_QUEUE_POS_INFO 수신 확인
- [ ] 탭B priority=255(Emergency) → 탭A Preemption Revoke 확인
- [ ] 30초 초과 → 서버에서 자동 FLOOR_REVOKE(max_duration) 확인
- [ ] 탭A 강제 닫기 → FLOOR_REVOKE(timeout or disconnect) + 탭B Grant 확인

### 통합 테스트 (integration_test.rs)

- [ ] Floor Request → Granted 시나리오
- [ ] Queue → 선행자 Release 후 자동 Grant 시나리오
- [ ] Preemption (Emergency) 시나리오
- [ ] Disconnect Revoke 시나리오

### net.rs 성능 개선 (메모리에 있음)

- [ ] `try_recv_from` mut 수정 선행
- [ ] `Iterator` lifetime 보강 (`Bytes::copy`)
- [ ] `num_cpus` 도입
- [ ] `DashMap` 전환 검토
- [ ] `SO_REUSEPORT` + `recvmmsg` 적용
- [ ] 부하 테스트 후 병목 확인 후 적용

### SRTP 릴레이 (Phase 1)

- [ ] DTLS keying material → `SrtpContext` 키 설치 (0.8.0 구현 확인 필요)
- [ ] 복호화된 RTP → 채널 내 다른 피어 relay
- [ ] Floor Taken 상태일 때만 릴레이 (holder → others)
- [ ] Floor Idle 상태에서 수신된 RTP는 drop 또는 버퍼

---

## [0.15.0] - 2026-02-28

### Floor Ping 방향 역전 (서버→클라이언트 → 클라이언트→서버) + error_code 통합

#### error.rs

- `LiveError::code(&self) -> u16` 메서드 추가 — 에러 코드를 enum 자체에 내장
- `protocol/error_code.rs` 완전 제거 (별도 파일/상수/`to_error_code()` 함수 삭제)
- 호출부 `to_error_code(&err)` → `err.code()` 로 교체

#### protocol/opcode.rs

- `client::FLOOR_PONG(32)` 제거 → `client::FLOOR_PING(32)` 추가 (C→S 생존 신호)
- `server::FLOOR_PING(116)` 제거 → `server::FLOOR_PONG(116)` 추가 (S→C 응답)

#### core.rs (FloorControl)

- `ping_seq`, `last_pong_at` 필드 제거 → `last_ping_at` 추가
- `next_ping_seq()`, `on_pong()`, `is_pong_timeout()` 제거
- `on_ping()` 추가 — 클라이언트 Ping 수신 시 `last_ping_at` 갱신
- `is_ping_timeout()` 추가 — `last_ping_at` 기준 타임아웃 판정
- `grant()` — `last_ping_at` 초기값을 Grant 시점으로 설정 (즉시 타임아웃 방지)

#### config.rs

- `FLOOR_PING_INTERVAL_MS`, `FLOOR_PONG_TIMEOUT_MS` 제거
- `FLOOR_PING_TIMEOUT_MS = 6_000` 추가 — 클라이언트 송신 주기 2초 기준 3배 여유

#### protocol/message.rs

- `FloorPongPayload(C→S, seq 포함)` → `FloorPingPayload(C→S, seq 없음)` 교체
- `FloorPingPayload(S→C, seq 포함)` → `FloorPongPayload(S→C, seq 없음)` 교체

#### protocol/floor.rs

- `handle_floor_pong()` 제거
- `handle_floor_ping(tx, user_id, channel_hub, packet)` 추가
  - `on_ping()` 으로 `last_ping_at` 갱신 후 `FLOOR_PONG(116)` 즉시 응답
- `run_floor_ping_task()` 제거 (별도 태스크 폐기)
- `check_floor_timeouts(user_hub, channel_hub)` 추가
  - zombie reaper에서 주기적으로 호출하는 일반 async 함수
  - `is_max_taken_exceeded()` 또는 `is_ping_timeout()` 시 Revoke 처리
  - Revoke cause: `"ping_timeout"` / `"max_duration"` 으로 구분

#### protocol/protocol.rs

- `FLOOR_PONG` dispatch → `FLOOR_PING` 으로 교체
- `error_code::to_error_code` import 제거

#### lib.rs

- `run_floor_ping_task` spawn 제거
- zombie reaper 4단계에 `check_floor_timeouts()` 호출 추가

#### client/app.js

- `state.floorPingSeq` → `state.floorPingTimer` 교체
- `case 116` 핸들러: `onFloorPing` → `onFloorPong` (수신 확인 로깅)
- `onFloorPing()` 삭제 → `_startFloorPing(channelId)` / `_stopFloorPing()` 추가
  - `_startFloorPing`: 2초 주기 `setInterval` 로 `op:32` 전송
  - `_stopFloorPing`: `clearInterval` 정리
- Ping 타이머 시작: `onFloorGranted` — `d.user_id === 나` 일 때
- Ping 타이머 정지: `pttStop(RELEASE 전)`, `onFloorIdle(wasMine)`, `onFloorRevoke(wasMine)`, `btn-leave`

---

## [0.14.0] - 2026-02-28

### SRTP/SRTCP 분리 복호화 + Floor Control 시그널링 버그 수정

#### media/srtp.rs

- `SrtpContext::decrypt_rtcp()` 추가
  - 내부 `Context::decrypt_rtcp()` 호출, 반환 `Vec<u8>`
  - 키 미설치 시 `KeyNotInstalled` 에러

#### media/net.rs

- **RTCP/RTP 분기 처리** (`handle_srtp`)
  - `byte1 >= 0xC8(200)` 이면 SRTCP — `decrypt_rtcp()` 경로로 분기
  - 이전: 모든 패킷을 `decrypt_rtp()`로 처리 → Chrome RTCP SR(byte1=0xC8) 첫 패킷 auth tag 실패
  - SRTCP는 통계용(SR/RR)이므로 복호화 후 drop (릴레이 없음)
- **MutexGuard Send 문제 수정** (`handle_srtp`)
  - `enum DecryptResult { Rtcp, Rtp(Vec<u8>), Err }` 도입
  - `ctx` MutexGuard를 블록 내에 완전히 격리 → 블록 종료 시 drop
  - `relay_to_channel().await` 진입 시점에 Guard 부재 보장
- **Floor Control 릴레이 게이트** (`relay_to_channel`)
  - `ChannelHub`를 파라미터로 추가
  - `FloorControlState::Taken && floor_taken_by == sender_user` 일 때만 릴레이
  - Floor Idle 또는 다른 사람이 holder면 trace 로그 후 drop
- `run_udp_relay()` / `handle_srtp()` 시그니처에 `channel_hub: Arc<ChannelHub>` 추가

#### protocol/floor.rs

- **FLOOR_TAKEN 시그널링 버그 수정**
  - Granted 케이스: `FLOOR_TAKEN`을 `broadcast_to(..., Some(user_id))` — 본인 제외
  - 이전: `broadcast_to(..., None)` → granted 본인도 FLOOR_TAKEN 수신 (중복)
  - Preempt 케이스 동일 수정
- **`dispatch_packets()` 시그니처 확장**
  - `Vec<(Option<String>, String)>` → `Vec<(Option<String>, Option<String>, String)>`
  - 3번째 필드: `exclude: Option<String>` — 브로드캐스트 시 제외할 user_id
- **`decide_next()` 반환 타입 일치**
  - Queue → Grant 시 FLOOR_TAKEN을 `(None, Some(next_user_id), json)` — holder 제외 전송
  - `PingAction::Revoke.packets` 타입도 3-튜플로 수정

#### lib.rs

- `media::run_udp_relay()` 호출에 `Arc::clone(&channel_hub)` 추가

---

## [0.13.0] - 2026-02-27

### Floor Control 구현 (MBCP TS 24.380 기반)

#### config.rs

- Floor Control 전용 상수 추가
  - `FLOOR_PING_INTERVAL_MS = 3_000` — 서버→holder Ping 주기
  - `FLOOR_PONG_TIMEOUT_MS = 5_000` — Pong 무응답 시 Revoke 기준
  - `FLOOR_MAX_TAKEN_MS = 30_000` — 최대 발언 점유 시간
  - `FLOOR_T100_MS`, `FLOOR_T101_MS = 3_000` — MBCP 타이머
  - `FLOOR_PRIORITY_EMERGENCY = 255`, `FLOOR_PRIORITY_IMMINENT_PERIL = 200`, `FLOOR_PRIORITY_DEFAULT = 100`

#### protocol/opcode.rs

- C→S opcode 추가: `FLOOR_REQUEST(30)`, `FLOOR_RELEASE(31)`, `FLOOR_PONG(32)`
- S→C opcode 추가: `FLOOR_GRANTED(110)`, `FLOOR_DENY(111)`, `FLOOR_TAKEN(112)`, `FLOOR_IDLE(113)`, `FLOOR_REVOKE(114)`, `FLOOR_QUEUE_POS_INFO(115)`, `FLOOR_PING(116)`

#### core.rs

- `User`에 `priority: u8` 필드 추가
- `UserHub::register()` 시그니처에 `priority: u8` 파라미터 추가
- `Channel`에 `floor: Mutex<FloorControl>` 필드 추가
- `FloorIndicator` enum 추가 (Normal / Broadcast / ImminentPeril / Emergency)
- `FloorControlState` enum 추가 (Idle / Taken)
- `FloorQueueEntry` 구조체 추가 (user_id, priority, indicator, queued_at)
- `FloorControl` 구조체 추가
  - `grant()` — 발언권 부여, 상태 Taken으로 전이
  - `clear_taken()` — Idle 복귀 공통 처리
  - `enqueue()` — priority 내림차순 삽입, 중복 user_id 갱신
  - `dequeue_next()` — 다음 대기자 꺼내기
  - `remove_from_queue()` — CHANNEL_LEAVE 등 연동
  - `can_preempt()` — Emergency는 무조건 true, 그 외 priority 비교
  - `next_ping_seq()` / `on_pong()` — Ping/Pong seq 관리
  - `is_pong_timeout()` / `is_max_taken_exceeded()` — 타임아웃 판정

#### protocol/message.rs

- `IdentifyPayload`에 `priority: Option<u8>` 추가
- Floor payload 타입 10개 추가
  - C→S: `FloorRequestPayload`, `FloorReleasePayload`, `FloorPongPayload`
  - S→C: `FloorGrantedPayload`, `FloorDenyPayload`, `FloorTakenPayload`, `FloorIdlePayload`, `FloorRevokePayload`, `FloorQueuePosInfoPayload`, `FloorPingPayload`
  - 공용: `FloorIndicatorDto` enum (serde rename_all = snake_case)

#### protocol/floor.rs (신규)

- Floor Control 도메인 로직 분리 (protocol.rs에서 독립)
- `handle_floor_request()` — Idle 즉시 Grant / Taken 시 Preemption 또는 Queue 진입
- `handle_floor_release()` — holder 검증 후 다음 후보 Grant 또는 Idle
- `handle_floor_pong()` — seq 검증 후 last_pong_at 갱신
- `run_floor_ping_task()` — 3초 주기 태스크, 최대발언시간/Pong타임아웃 감시
- `on_user_disconnect()` — 연결 종료 시 holder Revoke + 대기열 제거
- `decide_next()` (sync) — MutexGuard 보유 중 패킷 생성, Vec 반환
- `dispatch_packets()` (async) — lock 해제 후 패킷 전송
- **Send 안전 패턴**: `enum Action` / `decide_next` 로 MutexGuard를 await 포인트 이전에 drop

#### protocol/protocol.rs

- IDENTIFY 핸들러: `priority` 추출 후 `user_hub.register()` 전달
- `handle_floor_request/release/pong` dispatch 연결
- `cleanup()`: `on_user_disconnect()` 호출 추가

#### protocol.rs (mod)

- `pub mod floor` 추가

#### lib.rs

- `run_floor_ping_task` tokio::spawn 추가

#### client/index.html

- IDENTIFY 폼에 `priority` 입력 추가 (0~255, 기본 100)
- PTT 버튼: 오디오 트랙 즉시 활성화 → FLOOR_REQUEST/RELEASE WS 송신으로 교체
- Floor 수신 핸들러 추가: GRANTED/DENY/TAKEN/IDLE/REVOKE/QUEUE_POS/PING 7종
- FLOOR_PING 수신 시 자동 FLOOR_PONG 응답
- 오디오 트랙 활성화 시점: FLOOR_GRANTED 수신 시 (이전: PTT 누름 즉시)
- State 패널에 FLOOR/HOLDER/QUEUE 3행 추가
- 멤버 항목에 `▶ ON AIR` 배지 — FLOOR_TAKEN 수신 시 표시
- `setButtons('joined')`: ptt-btn 활성화 추가
- leave 처리에 Floor 상태 초기화 추가

---

## [0.12.0] - 2026-02-27

### 브라우저 E2E ICE+DTLS 연결 성공

#### Cargo.toml

- `rand = "0.8"` 추가 — ICE ufrag/pwd CSPRNG 생성
- `hmac = "0.12"`, `sha-1 = "0.10"`, `crc32fast = "1"` 추가 — STUN MESSAGE-INTEGRITY/FINGERPRINT

#### protocol/protocol.rs

- `detect_local_ip()` 추가: UDP 소켓으로 8.8.8.8:80 connect → local_addr() 조회 (라우팅 테이블 기반, 멀티홈 환경 대응)
- `random_ice_string()` 교체: xorshift → `rand::thread_rng()` CSPRNG, charset에서 `+/` 제거 (RFC 준수)
- `build_sdp_answer()` 반환 타입 변경: `String` → `(String, String, String)` (sdp, server_ufrag, server_pwd)
  - ufrag 길이 4 → 16자 (RFC 8445 범위 내, 충돌 방지)
  - `a=group:BUNDLE` 세션 헤더에 추가 (필수)
  - `m=` 포트를 offer 더미값 9 → `SERVER_UDP_PORT`로 교체
  - `c=IN IP4` 실제 서버 IP로 교체
  - `a=candidate` IP를 `detect_local_ip()` 결과로 교체
- `handle_channel_join()`: MediaPeerHub 등록 키를 client ufrag → server ufrag로 변경
  - STUN USERNAME = `server_ufrag:client_ufrag` 구조에 맞춤
  - `ice_pwd`를 Endpoint에 함께 저장

#### core.rs

- `Endpoint`에 `ice_pwd: String` 필드 추가
- `Endpoint::new()`, `MediaPeerHub::insert()` 시그니처에 `ice_pwd` 파라미터 추가

#### media/net.rs

- `parse_stun_username()`: `nth(1)` → `nth(0)` (client ufrag → server ufrag로 조회)
- `make_binding_response()`: `ice_pwd` 파라미터 추가
  - `MESSAGE-INTEGRITY`: HMAC-SHA1(key=ice_pwd) 추가 — 브라우저 필수 검증
  - `FINGERPRINT`: CRC32 XOR 0x5354554E 추가
  - RFC 5389 length 필드 단계별 업데이트 로직 구현
- `handle_stun()`: latch 후 `ep.ice_pwd` 꺼내서 `make_binding_response()`에 전달

#### 결과

- ICE: `checking` → `connected` → `completed` ✅
- DTLS: `connected` ✅
- SRTP 패킷 수신: Opus 73bytes @ 20ms 간격 ✅

---

## [0.11.0] - 2026-02-25

### SDP offer/answer 교환 구현 (CHANNEL_JOIN 확장)

#### protocol/message.rs

- `ChannelJoinPayload`에 `sdp_offer: Option<String>` 추가
- `ChannelJoinAckData`에 `sdp_answer: Option<String>` 추가

#### protocol/protocol.rs

- `AppState`에 `server_cert: Arc<ServerCert>` 추가
- `handle_channel_join()`에 `build_sdp_answer()` 호출 추가
- `build_sdp_answer()` 구현: offer 미디어 라인 미러링 + 서버 ICE/DTLS 정보 조립
- `random_ice_string()`: xorshift 기반 ICE ufrag/pwd 생성

#### lib.rs

- `AppState` 생성 시 `server_cert` 추가

---

## [0.10.0] - 2026-02-25

### 좀비 세션 Reaper 완성

#### config.rs

- `REAPER_INTERVAL_MS = 10_000` — reaper 실행 주기 (기존 HEARTBEAT_INTERVAL_MS와 분리)
- `DTLS_HANDSHAKE_TIMEOUT_MS = 10_000` — 핸드셰이크 최대 허용 시간

#### media/dtls.rs

- `start_dtls_handshake()` 에 `tokio::time::timeout` 추가
  - 타임아웃 시 `session_map.remove()` 호출 후 warn 로그
- `DtlsSessionMap::remove_stale()` 추가
  - `tx.is_closed()` 로 종료된 핸드셰이크 세션 감지
  - 제거된 `SocketAddr` 목록 반환

#### lib.rs

- `run_zombie_reaper()` 시그니처 확장: `ChannelHub` + `DtlsSessionMap` 추가
- 1단계: 좀비 User 제거 + 소속 채널 멤버에서 동시 제거
- 2단계: 좀비 Endpoint 제거 (UDP 패킷 없음)
- 3단계: 단절된 DTLS 세션 제거 (`remove_stale()`)
- reaper 간격을 `REAPER_INTERVAL_MS` 로 변경

---

## [0.9.0] - 2026-02-25

### Phase 2 완료 — SRTP 실제 암복호화 구현

#### media/srtp.rs

- `SrtpContext` 내부에 `Option<webrtc_srtp::context::Context>` 보관
- `install_key()` 에서 `Context::new(key, salt, Aes128CmHmacSha1_80, None, None)` 호출
- `decrypt()` / `encrypt()` 시그니처 변경: `&self` → `&mut self`, 반환 `Vec<u8>`
- 키 미설치 시 패스스루 제거 → `KeyNotInstalled` 에러 반환
- `init_srtp_contexts()` 에 `is_ready()` 검증 추가
- 테스트: `encrypt_decrypt_roundtrip` 추가 (5개 총)

#### media/net.rs

- `inbound_srtp.lock()` / `outbound_srtp.lock()` 시 `mut` 추가
- `.decrypt()` / `.encrypt()` 반환값이 `Vec<u8>` 이므로 `.to_vec()` 호출 제거

#### API 확정 (조사 결과)

- `webrtc_srtp::context::Context::decrypt_rtp()` 반환: `Result<bytes::Bytes>`
- `webrtc_srtp::context::Context::encrypt_rtp()` 반환: `Result<bytes::Bytes>`
- 에러 타입: `webrtc_srtp::Error` (`error` 모듈은 `pub(crate)` 라 전체 경로 사용)

---

## [0.8.0] - 2026-02-25

### Phase 2 완료 — DTLS-SRTP 키 도출 구현

#### media/dtls.rs

- `do_handshake()` TODO 블록 → 실제 구현으로 교체
- `dtls_conn.connection_state().await` 로 `State` 획득 (`DTLSConn.state` 는 `pub(crate)` 라 직접 접근 불가)
- `webrtc_util::KeyingMaterialExporter` 트레이트 import 후 `state.export_keying_material()` 호출
- RFC 5764 §4.2 레이아웃으로 60바이트 슬라이싱: `client_key(16) | server_key(16) | client_salt(14) | server_salt(14)`
- `init_srtp_contexts(endpoint, ...)` 호출로 Endpoint inbound/outbound SRTP 키 설치
- 불필요한 언더스코어 상수(`_SRTP_*`) 정리

#### 조사 결과 (2026-02-25)

- `export_keying_material()` 은 `dtls::state::State` 에 `KeyingMaterialExporter` 트레이트로 구현됨
- 트레이트 경로: `webrtc_util::KeyingMaterialExporter` (dtls 크레이트 내부가 아님)
- `context` 파라미터는 반드시 `&[]` — 비어있지 않으면 `ContextUnsupported` 에러

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
