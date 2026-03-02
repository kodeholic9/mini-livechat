# Changelog

All notable changes to this project will be documented in this file.

---

## [TODO] (다음 세션)

### 다음 과제

- [ ] **[P0] 모바일 오디오/비디오 수신 불가 디버깅** — play() 성공 로그는 찍히나 실제 소리 없음
  - 서버: 정상 확인 (SRTCP RR loss=0%, relay 정상)
  - 클라이언트: `[audio] play() 성공 (unmute)` 로그 확인됨
  - 의심: Cloudflare 캐시 (SDK 실제 v0.2.3, 단말 수신 v0.2.1) 또는 AudioContext 정책
  - **다음 확인 포인트**: 단말 크롬 캐시 완전 삭제 후 재테스트, `[audio] play() 성공 (ontrack)` 로그 확인
  - 영상도 동일 원인으로 추정 (오디오 해결 시 같이 해결될 가능성 높음)
- [ ] 멀티 원격 비디오 — 현재 remote-video 엘리먼트 1개, 다수 참여자 레이아웃 확장
- [ ] E2E 비디오 테스트 (카메라 환경 필요)

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

### tokio features 최적화

- [ ] `features = ["full"]` → 실제 사용 feature만 명시 (빌드 시간 단축)
  - 필요 feature: `rt-multi-thread`, `macros`, `net`, `sync`, `time`, `io-util`
  - 제거 가능: `signal`, `process`, `fs` 등 미사용 feature

### net.rs 성능 개선 (메모리에 있음)

- [ ] `try_recv_from` mut 수정 선행
- [ ] `Iterator` lifetime 보강 (`Bytes::copy`)
- [ ] `num_cpus` 도입
- [ ] `DashMap` 전환 검토
- [ ] `SO_REUSEPORT` + `recvmmsg` 적용
- [ ] 부하 테스트 후 병목 확인 후 적용

### Conference 모드 추가 (SFU 다자통화)

- [x] Step 1: Channel에 mode 필드 추가 (0.20.4)
- [x] Step 2: relay_to_channel()에서 mode별 분기 (0.20.4)
- [x] Step 3: CHANNEL_JOIN ACK에 mode 필드 추가 (0.20.5)
- [x] Step 4: SDK Conference 모드 지원 (v0.4.0~0.4.1)
  - _channelMode 상태, isConference getter
  - DTLS connected 시 Conference 자동 unmute
  - requestFloor/releaseFloor Conference noop
  - getChannelInfo() + channel:info 이벤트
  - switchCamera() 전면/후면 전환 (PTT/Conference 공통)
- [x] Step 5: Conference UI (Google Meet 스타일 그리드)
  - PTT/Conference 하단 컨트롤 바 통일
  - 입퇴장 시 CHANNEL_INFO 재조회로 그리드 갱신
  - 카메라 1개 이하 시 전환 버튼 disable

- [ ] **[P0] Unified Plan SDP 재협상 (N:N 비디오 핵심)**
  - 현재 문제: 1:1 PeerConnection 구조라 상대 비디오 SSRC가 SDP에 없어 브라우저가 drop
  - 방식: Unified Plan + addTransceiver per peer + re-offer/answer (mediasoup/Jitsi 방식)
  - 프로토콜: RENEGOTIATE opcode 추가 (C→S: re-offer, S→C: re-answer)
  - 서버: 동적 SDP answer 생성 (peer별 recvonly m-line + SSRC 매핑)
  - SDK: peer 입퇴장 시 addTransceiver + createOffer + re-negotiation
  - 릴레이: 발신자 SSRC → 수신자 m-line 매핑
  - 부장님 설계 수정사항 반영 필요 (새 대화에서 진행)
- [ ] SSRC 충돌 감지/처리
- [ ] RTCP 포워딩 (Conference 품질 피드백)

### SRTP 릴레이 (Phase 1)

- [ ] DTLS keying material → `SrtpContext` 키 설치 (0.8.0 구현 확인 필요)
- [ ] 복호화된 RTP → 채널 내 다른 피어 relay
- [ ] Floor Taken 상태일 때만 릴레이 (holder → others)
- [ ] Floor Idle 상태에서 수신된 RTP는 drop 또는 버퍼

---

## [0.20.6] - 2026-03-03

### Conference SSRC Rewrite 기반 구축 — consumer SSRC 생성/매핑/relay rewrite

#### core/media_peer.rs

- `TrackKind`에 `Hash`, `Eq` derive 추가 (HashMap 키로 사용)
- `ConsumerSsrcKey` 구조체 추가 — (receiver, sender, kind) 복합 키
- `MediaPeerHub`에 `consumer_ssrc`, `ssrc_relay_map` 필드 추가
- `get_or_create_consumer_ssrc()` — 서버가 consumer SSRC 할당/조회
- `rebuild_relay_map()` — sender SSRC → Vec<(receiver, consumer_ssrc)> 역방향 맵
- `get_relay_targets()` — relay 핫패스 O(1) 조회
- `remove_consumer_ssrc_for_user()` — 퇴장 시 정리

#### protocol/protocol.rs

- `handle_renegotiate()` — 원본 producer SSRC 대신 consumer SSRC를 SDP answer에 삽입
- `handle_renegotiate()` — answer 전송 후 `rebuild_relay_map()` 호출
- `handle_channel_leave()` / `cleanup()` — consumer SSRC 정리 + relay map 재구축 추가

#### media/net.rs

- `relay_to_channel()` Conference SSRC rewrite 경로 추가
  - sender SSRC로 relay map 조회 → 각 receiver에 consumer SSRC로 RTP header rewrite
  - relay map 없으면 기존 브로드캠스트 fallback (PTT 호환)

#### core.rs

- `ConsumerSsrcKey` re-export 추가

---

## [0.20.5] - 2026-03-02

### Conference 모드 지원 — CHANNEL_JOIN ACK에 mode 필드 추가

#### protocol/message.rs

- `ChannelJoinAckData`에 `mode: String` 필드 추가

#### protocol/protocol.rs

- `handle_channel_join()` ACK 응답에 `channel.mode.to_string()` 전달
  - SDK가 JOIN 응답에서 채널 모드를 인지할 수 있도록 함

---

## [0.20.4] - 2026-03-02

### Conference 모드 지원 — Step 1: ChannelMode 추가

PTT(무전) 외에 Conference(다자통화 SFU) 모드를 채널 단위로 선택 가능하도록 기반 구조 추가.

#### core/channel.rs

- `ChannelMode` enum 신규: `PTT` | `Conference` (serde rename_all lowercase)
- `ChannelMode::from_str_lossy()` — 문자열 변환, 알 수 없는 값이면 기본값 PTT
- `Channel` 구조체에 `mode: ChannelMode` 필드 추가
- `Channel::is_ptt()` 헬퍼 추가
- `ChannelHub::create()` 시그니처에 `mode` 파라미터 추가
- 테스트 3개 추가: `create_conference_channel`, `channel_mode_from_str_lossy`, `channel_mode_default_is_ptt`

#### core.rs

- re-export에 `ChannelMode` 추가

#### protocol/message.rs

- `ChannelCreatePayload`에 `mode: Option<String>` 추가 (하위 호환: 없으면 기본 ptt)
- `ChannelSummary`, `ChannelInfoData`에 `mode: String` 필드 추가

#### protocol/protocol.rs

- `handle_channel_create()` — payload.mode 파싱 후 `ChannelMode::from_str_lossy()` 변환
- `handle_channel_list()`, `handle_channel_info()` 응답에 mode 반영
- ACK 응답에 `mode` 필드 포함

#### config.rs

- `PRESET_CHANNELS` 튜플에 mode 필드 추가: `(&str, &str, &str, &str, usize)`
- 기존 3개 채널 모두 `"ptt"` 모드로 설정

#### lib.rs

- 사전 정의 채널 생성 루프에서 mode 파싱 및 전달

#### http/dto.rs

- `ChannelSummary`, `ChannelDetail`, `AdminChannelSummary`, `AdminChannelDetail`에 `mode: String` 필드 추가

#### http/channel.rs, http/admin.rs

- 모든 채널 조회 응답에 `mode` 반영

#### media/net.rs

- `relay_to_channel()` — 모드별 릴레이 게이트 분기
  - PTT: 기존 Floor Control 체크 유지 (holder만 릴레이)
  - Conference: floor check 스킵, 모든 발신자 통과

---

## [0.20.3] - 2026-02-28

### Admin Floor Revoke 클라이언트 통지 추가

#### src/http/admin.rs

- `admin_floor_revoke()` — Floor 강제 회수 시 클라이언트 통지 추가
  - 이전: Floor 상태만 초기화, 클라이언트는 모르는 상태
  - 수정: holder에게 FLOOR_REVOKE(cause="admin_revoke") 전송 + 전체 멤버에게 FLOOR_IDLE 브로드캠스트

---

## [0.20.2] - 2026-02-28

### CORS 허용

#### Cargo.toml

- `tower-http = { version = "0.6", features = ["cors"] }` 추가

#### src/lib.rs

- `CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any)` 적용
  - Admin 대시보드, PTT 클라이언트 로컬 접속 시 CORS 이슈 해소
  - 내부 네트워크 전용 서버이므로 전체 허용

---

## [0.20.1] - 2026-02-28

### Floor Control 버그 수정 — non-holder RELEASE 시 큐 미제거

#### src/protocol/floor.rs

- `handle_floor_release()` — non-holder가 FLOOR_RELEASE를 보낸 경우 `remove_from_queue()` 호출 추가
  - 이전: warn 로그만 찍고 무시 → 큐에 좀비로 남아 holder가 release하면 취소한 사용자에게 자동 GRANTED
  - 수정: 큐에서 깔끔하게 제거 후 return (상태 전이 없음)
  - warn! → trace! 레벨 변경 (정상 동작이므로)

---

## [0.20.0] - 2026-02-28

### 리팩터링 — 모듈 분리 + 단위 테스트 71개

기능 확장 시 최소 파일만 수정하도록 모노리식 파일들을 도메인별로 분리.
로직 변경 없이 파일 분리 + re-export + import 경로만 변경.
Rust 2018 edition 스타일 (`core.rs` + `core/` 디렉터리) 적용, `mod.rs` 미사용.

#### core.rs 분리 (18.2KB → 4개 파일)

- `core.rs` → 서브모듈 선언 + re-export
- `core/user.rs` — UserHub, User, BroadcastTx
- `core/channel.rs` — ChannelHub, Channel
- `core/floor.rs` — FloorControl, FloorControlState, FloorIndicator, FloorQueueEntry
- `core/media_peer.rs` — MediaPeerHub, Endpoint, Track, TrackKind

#### http.rs 분리 (18.9KB → 5개 파일)

- `http.rs` → 서브모듈 선언 + re-export
- `http/state.rs` — HttpState
- `http/dto.rs` — 응답 DTO 17개
- `http/admin.rs` — Admin REST 핸들러 8개
- `http/channel.rs` — 일반 채널 조회 핸들러
- `http/trace.rs` — Trace SSE 스트림 핸들러

#### protocol/protocol.rs SDP 분리

- `protocol/sdp.rs` 신규 — `build_sdp_answer()`, `detect_local_ip()`, `random_ice_string()`
- `protocol/protocol.rs`에서 ~150줄 제거, `sdp.rs` import로 대체

#### Floor 코드 중복 제거

- `check_floor_timeouts()` + `check_floor_timeouts_traced()` 복사본 2개
  → `check_floor_timeouts(..., trace_hub: Option<&Arc<TraceHub>>)` 1개로 통합
- ~40줄 중복 코드 제거

#### lib.rs → reaper.rs 분리

- `run_zombie_reaper()` → `reaper.rs` 독립 모듈
- `lib.rs`는 모듈 선언 + `run_server()` 오케스트레이션만 담당

#### 단위 테스트 71개 작성 (cargo test 전체 통과)

- `core/user.rs` — 7개 (register/unregister/count/duplicate/all_users/touch/zombie)
- `core/channel.rs` — 8개 (create/duplicate/remove/add_member/capacity/dup_member/remove_member/floor_count)
- `core/floor.rs` — 14개 (상태전이/enqueue우선순위/중복enqueue/remove/position/preempt 3종/ping/timeout 3종)
- `core/media_peer.rs` — 9개 (insert/latch/latch_unknown/remove/channel_filter/count/track_dedup/address/zombie)
- `error.rs` — 6개 (코드범위 1xxx/2xxx/3xxx/9xxx + display + 범위검증)
- `protocol/sdp.rs` — 14개 (ice_string 3개 + SDP answer 8개 + BUNDLE 2개 + detect_ip 1개)
- `trace.rs` — 4개 (no_subscriber/subscribe/multi_subscriber/json직렬화)
- `media/srtp.rs` — 5개 (기존)
- `media/net.rs` — 4개 (기존)

---

## [0.19.0] - 2026-02-28

### 비디오 지원 추가 (BUNDLE 확장)

#### src/protocol/protocol.rs

- `build_sdp_answer()` 전면 리팩토링
  - 단일 audio 하드코딩 → `MediaSection` 구조체 기반 범용 파서로 교체
  - offer의 `m=` 섹션을 순서대로 수집한 뒤 audio/video 모두 동일 패턴으로 미러링
  - BUNDLE `a=group` 구성 시 mid 목록을 offer 순서대로 조립
  - 서버 코드 변경 없이 offer에 `m=video` 있으면 자동 수락

#### client/index.html

- PTT 섹션에 비디오 토글 (ON/OFF 슬라이더) 추가
  - JOIN 전에 설정, 커넥티던 후 변경 시 다음 JOIN에서 적용 안내
- 좌측 쿼름 하단에 VIDEO 영역 추가 (toggle ON 시에만 표시)
  - LOCAL: 자신 카메라 프리뷰 (`<video autoplay muted>`)
  - REMOTE: Floor holder 비디오 수신 (`<video autoplay>`)

#### client/app.js

- `state.videoEnabled`, `state.videoSender` 추가
- `setAudioTransmission()` → `setMediaTransmission()` 로 확장
  - 오디오 + 비디오 sender 동시 `replaceTrack()` 제어
  - `setAudioTransmission()`은 하위 호환 alias로 유지
- `initVideoToggle()` 함수 추가 — 체크박스 변경 시 슬라이더 UI + state 동기화
- `setupWebRTC()` 확장
  - `getUserMedia`에 `video: state.videoEnabled ? { width:640, height:480, fps:15 } : false` 조건 적용
  - 비디오 트랙이 있으면 `pc.addTrack()` 후 초기 `replaceTrack(null)` 차단
  - 로컈 비디오를 `local-video` 엘리먼트에 연결
  - `createOffer`에 `offerToReceiveVideo: state.videoEnabled` 전달
- `ontrack()` 확장 — `kind === 'video'` 시 `remote-video` 엘리먼트에 연결
- `onFloorGranted()` — `setMediaTransmission(true)` 로 오디오+비디오 동시 상향 시작
- `onFloorIdle()` / `onFloorRevoke()` — `remote-video.srcObject = null` 추가
- 언마운트 시 `local/remote-video srcObject` 클리어

---

## [0.18.0] - 2026-02-28

### Floor Control 버그 수정 2종

#### src/protocol/protocol.rs

- `handle_channel_join()` — 신규 입장자에게 FLOOR_TAKEN 전송 추가
  - 입장 시체널이 Taken 상태면 신규 입장자 혹시만한테 `FLOOR_TAKEN` 전송
  - BUNDLE 환경에서 SRTP는 흐르지만 클라이언트 UI가 idle 로 남는 문제 해소
  - `MutexGuard`가 await를 걸치면 `Send` 불만족 → 동기 블록에서 패킷 문자열만 추출, Guard는 블록 끝에서 drop

#### client/app.js

- `pttStop()` — `queued` 상태에서 PTT OFF 시 `FLOOR_RELEASE` 미전송 버그 수정
  - `requesting` / `queued` 분기 추가, 모든 경로에서 서버에 `FLOOR_RELEASE` 전송
- `onFloorIdle()` — `wasQueued` 상태를 수정 전에 케시, `pttActive` 강제 리셋 추가
  - `state.floorState = 'idle'` 후 `=== 'queued'` 체크로 항상 false 되던 로직 버그 해소
- `onFloorRevoke()` — `floorHolder` null 정리 추가, wasMine/not-wasMine 경로 명확화

---

## [0.17.0] - 2026-02-28

### lctrace 실시간 시그널링 관샼 CLI

#### Cargo.toml

- `tokio-stream = "0.1"` (sync feature) 추가 — `BroadcastStream` SSE 스트림용
- `[[bin]] name = "lctrace"` 선언 추가

#### src/trace.rs (신규)

- `TraceHub` — `tokio::sync::broadcast` 기반 이벤트 버스
  - `publish()` — 구독자없으면 조용히 비워나감 (O(1), 서버 성능 무영향)
  - `subscribe()` — SSE 연결마다 호출, `BroadcastReceiver` 반환
- `TraceEvent` — 시그널링 이벤트 구조체 (`ts`, `dir`, `channel_id`, `user_id`, `op`, `op_name`, `summary`)
- `TraceDir` — `In` (C→S) / `Out` (S→C) / `Sys` (서버 내부)

#### src/lib.rs

- `pub mod trace` 선언 추가
- `TraceHub::new()` 생성, `AppState` 및 `HttpState`에 주입
- `run_zombie_reaper` 시그니처에 `trace_hub` 추가
- `check_floor_timeouts` → `check_floor_timeouts_traced` 대체
- `/trace` / `/trace/{channel_id}` SSE 라우트 마운트

#### src/protocol/protocol.rs

- `AppState`에 `trace_hub: Arc<TraceHub>` 요소 추가
- `publish_in_event()` 유틸 함수 추가 — C→S 수신 패킷을 한 줄로 publish (HEARTBEAT 제외)
- `op_meta_in()` — opcode → (이름, 요약) 매핑
- CHANNEL_JOIN 핸들러에 `TraceDir::Sys` 입장 이벤트 publish 추가
- FLOOR_REQUEST / FLOOR_RELEASE 호출에 `trace_hub` 인수 추가

#### src/protocol/floor.rs

- `handle_floor_request` / `handle_floor_release` 시그니처에 `trace_hub` 추가
- `handle_floor_request` — Granted / Preempt / Queued 세 가지 경로에 이벤트 publish
- `handle_floor_release` — RELEASE→IDLE 이벤트 publish
- `check_floor_timeouts_traced()` 신규 — REVOKE 시간초과 이벤트 publish 포함 버전

#### src/http.rs

- `HttpState`에 `trace_hub: Arc<TraceHub>` 요소 추가
- `HttpState::new()` 시그니처에 `trace_hub` 추가
- `trace_stream()` 핸들러 추가 — SSE `text/event-stream`
  - `BroadcastStream` 기반 스트림, 15초 keep-alive
  - channel_id 라우트 파라미터 유무에 따라 전체 or 특정 채널 필터
  - Lagged 에러(bfq속자 느림) 는 `None`으로 skip — 서버 성능 무영향

#### src/bin/trace.rs (신규)

- `lctrace` CLI 바이너리 (reqwest blocking + SSE chunked read)
- clap 옵션: `--host`, `--port`, `--filter`, `[CHANNEL_ID]`
- 코드 이벤트 콜러 옵션:
  - `FLOOR_GRANTED` → 초록 bold
  - `FLOOR_REVOKE` / `FLOOR_DENY` → 빨간색 bold
  - `FLOOR_*` → 노란색
  - `*JOIN` / `*LEAVE` → 청록색
  - `IDENTIFY` → 자주색
- 방향 콜러: `↓ C→S` (blue) / `↑ S→C` (green) / `· SYS` (yellow)
- 서버 측 필터 + 클라이언트 측 `--filter` 복잡 필터 조합 가능

---

## [0.16.0] - 2026-02-28

### 운영 관리 CLI (lcadmin) + PTT 토글 + 채널 개편

#### Cargo.toml

- `[[bin]] name = "lcserver"` / `[[bin]] name = "lcadmin"` 선언
- `reqwest = "0.12"` (json, blocking feature) 추가 — lcadmin HTTP 클라이언트
- `tabled = "0.17"` 추가 — 터미널 테이블 렌더링
- `colored = "2"` 추가 — 터미널 컬러 출력

#### src/bin/admin.rs (신규)

- `lcadmin` 운영 관리 CLI 바이너리 신규 작성
- `clap` subcommand 구조: `status` / `users` / `channels` / `peers` / `floor-revoke`
- `--host` / `--port` 옵션으로 원격 서버 접속 지원
- `tabled` + `colored` 기반 터미널 컬러 테이블 출력
- `reqwest::blocking` HTTP 클라이언트 (동기, 별도 런타임 불필요)
- `deser_opt_string` — `Option<String>` JSON 필드를 `"-"` 폴백 String으로 역직렬화

#### src/http.rs

- `HttpState`에 `start_time_ms: u64` 추가 — 서버 시작 시각, uptime 계산용
- `HttpState::new()` 생성자 추가 — `SystemTime::now()` 기반 시작 시각 캡처
- Admin 조회 엔드포인트 추가
  - `GET /admin/status` — uptime, user/channel/peer 수, Floor 활성 채널 수
  - `GET /admin/users` — User 전체 목록 (user_id, priority, idle_secs)
  - `GET /admin/users/{user_id}` — User 상세 + 소속 채널 목록
  - `GET /admin/channels` — Channel 전체 목록 (Floor 상태, holder, 대기열 수)
  - `GET /admin/channels/{channel_id}` — Channel 상세 (대기열, peer 목록 포함)
  - `GET /admin/peers` — Endpoint 전체 목록 (address, idle_secs, SRTP 상태)
  - `GET /admin/peers/{ufrag}` — Endpoint 상세 (tracks 포함)
- Admin 조작 엔드포인트 추가
  - `POST /admin/floor-revoke/{channel_id}` — Floor 강제 Idle 복귀 (queue 포함 초기화)
- 기존 `/channels`, `/channels/{id}` 라우터를 admin_router로 통합

#### src/core.rs

- `UserHub::all_users()` 추가 — 전체 User 목록 반환 (admin 조회용)
- `UserHub::count()` 추가 — 현재 접속 User 수
- `ChannelHub::count()` 추가 — 현재 채널 수
- `ChannelHub::count_floor_taken()` 추가 — Floor Taken 상태 채널 수
- `MediaPeerHub::get_by_ufrag()` 추가 — ufrag 기반 Endpoint 단건 조회
- `MediaPeerHub::all_endpoints()` 추가 — 전체 Endpoint 목록 반환
- `MediaPeerHub::count()` 추가 — 현재 Endpoint 수

#### src/lib.rs

- `pub mod http` 선언 추가
- `HttpState::new()` 생성 및 admin 라우터 mount
- 기존 `/channels` 라우터를 admin_router에 통합 (merge)
- `routing::post` import 추가

#### config.rs

- 사전 생성 채널 5개 → 3개로 변경
  - `CH_0001 / 0001 / 📢 영업/시연 / 20명`
  - `CH_0002 / 0002 / 🤝 스스 파트너스 / 20명`
  - `CH_0003 / 0003 / 🏠 동천 패밀리 / 20명`

#### client/app.js

- PTT 버튼 동작 방식 변경: Hold(누르는 동안) → Toggle(클릭 시 전환)
  - `mousedown/mouseup/mouseleave` 이벤트 제거 → `onclick` 단일 이벤트
  - `Space` keyup 제거 → keydown 단일 토글
  - 모바일: `touchend` 제거 → `touchstart` 토글
- 채널 목록 수신 시 `CH_0001` 기본 선택 (이전 선택 채널 유지 우선)

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
