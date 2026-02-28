# Mini LiveChat

초고성능 무전(PTT) 및 실시간 미디어 릴레이를 위한 경량 백엔드 서버 엔진입니다.  
Rust + Tokio + Axum 기반으로 엣지 디바이스 환경에서도 안정적으로 동작하도록 설계되었습니다.

---

## 아키텍처 개요

```
클라이언트 (WebSocket)
    │
    ▼
WebSocket Gateway (Axum, TCP)
    │
    ├── IDENTIFY     → UserHub 등록 (라우팅 테이블)
    ├── CHANNEL_JOIN → ChannelHub 멤버 등록 + MediaPeerHub ICE ufrag 등록 + SDP answer 생성
    ├── FLOOR_REQUEST → FloorControl 상태머신 (Grant / Queue / Preempt)
    ├── MESSAGE_CREATE → ChannelHub 멤버 목록 → UserHub.broadcast_to()
    └── CHANNEL_LEAVE / WS 종료 → 자동 클린업

HTTP REST API (Axum, TCP — 동일 포트)
    ├── GET  /channels, /channels/{id}          일반 조회
    └── GET|POST /admin/*                       운영 관리 (lcadmin CLI 연동)

UDP 미디어 릴레이 (net.rs, ICE Lite + DTLS-SRTP)
    │
    ├── STUN  → ICE ufrag 파싱 → MediaPeerHub latch → Binding Response
    ├── DTLS  → 핸드셰이크 → keying material 추출 → SRTP 키 설치
    └── SRTP  → by_addr O(1) 조회 → 복호화 → Floor 게이트 → 채널 릴레이
```

### 상태 관리 3계층

| 허브 | 키 | 역할 |
|---|---|---|
| `UserHub` | user_id | WS 세션 + 브로드캐스트 라우팅 테이블 |
| `ChannelHub` | channel_id | 채널 정의 + 멤버 목록 + FloorControl 상태 |
| `MediaPeerHub` | ufrag / SocketAddr | 미디어 릴레이 핫패스 (O(1) 조회) |

### 설계 원칙

- **제어/데이터 평면 분리** — WebSocket(시그널링)과 UDP(미디어)를 완전 분리
- **ICE Lite** — candidate 단일 고정 IP, 전체 ICE 협상 없이 latch
- **핫패스 O(1)** — SocketAddr → Endpoint by_addr 맵으로 UDP 수신 즉시 피어 조회
- **Floor 게이트** — SRTP 릴레이는 Floor Taken 상태의 holder 패킷만 통과
- **Lock 안전 패턴** — MutexGuard를 await 포인트 이전에 반드시 drop
- **좀비 감지** — `last_seen` / `last_ping_at` 기반 타임아웃, zombie reaper 주기 정리

---

## 바이너리

| 바이너리 | 설명 |
|---|---|
| `lcserver` | 미디어 릴레이 서버 본체 |
| `lcadmin` | 운영 관리 CLI — HTTP REST API 기반 원격 조회/조작 |
| `lctrace` | 실시간 시그널링 관찰 CLI — SSE 기반 이벤트 스트림 |

---

## 빌드 및 실행 (개발 환경)

```bash
# 빌드 (디버그)
cargo build

# 빌드 (릴리즈)
cargo build --release

# 서버 실행 (기본값)
cargo run --bin lcserver

# CLI 인자로 설정 주입
cargo run --bin lcserver -- --port 8080 --udp-port 10000

# 외부 공인 IP 수동 지정 (도커/NAT 환경)
cargo run --bin lcserver -- --port 8080 --udp-port 10000 --advertise-ip 203.0.113.10

# 로그 레벨 설정
RUST_LOG=info cargo run --bin lcserver
RUST_LOG=trace cargo run --bin lcserver -- --port 8080 --udp-port 10000
```

### 서버 CLI 인자

| 인자 | 기본값 | 설명 |
|---|---|---|
| `--port` | `8080` | WebSocket + HTTP REST 공용 TCP 포트 |
| `--udp-port` | `10000` | UDP 미디어 릴레이 포트 |
| `--advertise-ip` | 자동 감지 | SDP candidate에 광고할 IP. 생략 시 라우팅 테이블로 로컬 IP 자동 감지 |

> **NAT / 도커 환경**: 컨테이너 내부 IP와 외부 접근 IP가 다를 경우 `--advertise-ip`로 공인 IP를 명시해야 WebRTC ICE가 정상 동작합니다.

### 환경변수

| 변수 | 기본값 | 설명 |
|---|---|---|
| `LIVECHAT_SECRET` | `changeme-secret` | IDENTIFY 토큰 검증용 Secret Key. 운영 환경에서는 반드시 교체할 것 |
| `RUST_LOG` | — | 로그 레벨 (`error` / `warn` / `info` / `debug` / `trace`) |

---

## 배포 및 운영 (서버 환경)

서버 환경(예: Raspberry Pi, Linux 서버) 배포는 `deploy-livechat.sh` 스크립트로 관리합니다.
Git clone → cargo build --release → nohup 기동 방식이며, 바이너리 백업 및 로그 관리를 포함합니다.

### 초기 설치 (1회)

```bash
mkdir -p ~/livechat && cd ~/livechat

# 배포 스크립트 다운로드
curl -O https://raw.githubusercontent.com/kodeholic9/mini-livechat/main/deploy-livechat.sh
chmod +x deploy-livechat.sh

# Rust 툴체인 설치 + 빌드 의존성 + 초기 빌드
./deploy-livechat.sh setup

# 환경변수 설정 (LIVECHAT_SECRET 필수 변경)
vi .env
```

`setup` 완료 후 디렉토리 구조:

```
~/livechat/
├── deploy-livechat.sh       ← 배포/운영 스크립트
├── src/                     ← git clone + cargo build 작업 디렉토리
├── bin/
│   ├── lcserver             ← release 바이너리
│   ├── lcadmin
│   └── lctrace
├── backup/                  ← 이전 바이너리 백업 (최근 5개 유지)
├── logs/                    ← stdout/stderr 로그 (날짜별)
└── .env                     ← 환경변수 (LIVECHAT_SECRET, RUST_LOG 등)
```

### 일상 운영

```bash
# 서버 시작/종료/재시작
./deploy-livechat.sh start
./deploy-livechat.sh stop
./deploy-livechat.sh restart

# 상태 확인 (PID, CPU, 메모리, 바이너리 정보)
./deploy-livechat.sh status

# 로그 실시간 확인
./deploy-livechat.sh log

# 새 버전 배포 (백업 → git pull → build --release → restart)
./deploy-livechat.sh patch
```

### .env 예시

```bash
RUST_LOG=info
LIVECHAT_SECRET=my_production_secret_key
# ADVERTISE_IP=192.168.1.100
```

---

## lcadmin — 운영 관리 CLI

서버가 실행 중인 상태에서 별도 터미널로 실행합니다.  
HTTP REST API를 통해 조회/조작하므로 서버 재시작 없이 실시간 확인 가능합니다.

```bash
# 기본 사용법 (로컬 서버 8080 포트)
cargo run --bin lcadmin -- <command>

# 원격 서버 접속
cargo run --bin lcadmin -- --host 192.168.1.10 --port 8080 <command>

# 릴리즈 빌드 후 직접 실행
lcadmin --host 127.0.0.1 --port 8080 <command>
```

### 조회 명령

```bash
# 서버 상태 요약 (uptime, 접속자 수, Floor 활성 채널 수)
lcadmin status

# User 전체 테이블 (user_id, 우선순위, 마지막 heartbeat 이후 경과)
lcadmin users

# User 상세 (소속 채널 포함)
lcadmin users swift_falcon_4821

# Channel 전체 테이블 (Floor 상태, holder, 대기열 수)
lcadmin channels

# Channel 상세 (멤버 목록, Floor 대기열, Peer 목록)
lcadmin channels CH_0001

# Endpoint(Peer) 전체 테이블 (ufrag, address, SRTP 준비 여부)
lcadmin peers

# Endpoint 상세 (tracks 포함)
lcadmin peers abcd1234efgh5678
```

### 조작 명령

```bash
# Floor 강제 revoke (holder + 대기열 모두 초기화, Idle 복귀)
lcadmin floor-revoke CH_0001
```

### 실행 예시

```
$ lcadmin status

  mini-livechat Server Status
  ────────────────────────────────────
  Uptime:          0h 12m 34s
  Users:           3
  Channels:        3
  Peers:           2
  Floor Active:    1

$ lcadmin channels

 CHANNEL ID  FREQ  NAME              MEMBERS  CAP  FLOOR    HOLDER              Q
 CH_0001     0001  📢 영업/시연      2        20   ● TAKEN  swift_falcon_4821   0
 CH_0002     0002  🤝 스스 파트너스  1        20   ○ idle   -                   0
 CH_0003     0003  🏠 동천 패밀리    0        20   ○ idle   -                   0

$ lcadmin channels CH_0001

  Channel: CH_0001 [0001] 📢 영업/시연
  ────────────────────────────────────────────────
  Capacity:          2/20
  Created:           day+20511 09:30:00 UTC
  Floor:             ● TAKEN (holder: swift_falcon_4821, 8s 경과, priority: 100)

  Members
    · swift_falcon_4821
    · brave_wolf_1234

  Peers
   UFRAG             USER ID             CHANNEL   IDLE(s)  SRTP
   abcd1234efgh5678  swift_falcon_4821   CH_0001   0        true
   wxyz9876mnop5432  brave_wolf_1234     CH_0001   1        true

$ lcadmin floor-revoke CH_0001

  Floor Revoke OK channel=CH_0001 revoked_from=swift_falcon_4821
```

---

## 프로토콜

디스코드 Gateway 스타일 opcode 기반 패킷 구조를 채택합니다.

```json
{ "op": 11, "d": { "channel_id": "CH_0001", "ssrc": 12345, "ufrag": "abcd1234" } }
```

### Client → Server Opcodes

| op | 이름 | 설명 |
|---|---|---|
| 1 | HEARTBEAT | 연결 유지 |
| 3 | IDENTIFY | 인증 (user_id, token, priority) |
| 10 | CHANNEL_CREATE | 채널 생성 (channel_id, freq, channel_name) |
| 11 | CHANNEL_JOIN | 채널 참여 (ssrc, ufrag, sdp_offer) |
| 12 | CHANNEL_LEAVE | 채널 나가기 |
| 13 | CHANNEL_UPDATE | 채널 정보 수정 |
| 14 | CHANNEL_DELETE | 채널 삭제 |
| 15 | CHANNEL_LIST | 채널 목록 조회 |
| 16 | CHANNEL_INFO | 채널 상세 조회 |
| 20 | MESSAGE_CREATE | 채팅 메시지 전송 |
| 30 | FLOOR_REQUEST | PTT — 발언권 요청 |
| 31 | FLOOR_RELEASE | PTT — 발언권 반납 |
| 32 | FLOOR_PING | holder 생존 신호 (GRANTED 후 2초 주기 자율 전송) |

### Server → Client Opcodes

| op | 이름 | 설명 |
|---|---|---|
| 0 | HELLO | 연결 직후 heartbeat 주기 안내 |
| 2 | HEARTBEAT_ACK | HEARTBEAT 수신 확인 |
| 4 | READY | IDENTIFY 성공, 세션 정보 전달 |
| 100 | CHANNEL_EVENT | 채널 멤버 변동 브로드캐스트 (join/leave/update/delete) |
| 101 | MESSAGE_EVENT | 채팅 메시지 브로드캐스트 |
| 110 | FLOOR_GRANTED | 발언권 허가 (holder 본인에게만) |
| 111 | FLOOR_DENY | 발언권 거부 |
| 112 | FLOOR_TAKEN | 누군가 발언 중 (holder 제외 채널 전체 브로드캐스트) |
| 113 | FLOOR_IDLE | 채널 유휴 상태 (채널 전체 브로드캐스트) |
| 114 | FLOOR_REVOKE | 발언권 강제 회수 (preempted / ping_timeout / max_duration / disconnect) |
| 115 | FLOOR_QUEUE_POS_INFO | 대기열 진입 확인 (position, size) |
| 116 | FLOOR_PONG | FLOOR_PING 응답 |
| 200 | ACK | 요청 성공 응답 |
| 201 | ERROR | 에러 응답 (code + reason) |

### 에러 코드

| 범위 | 설명 |
|---|---|
| 1xxx | 연결/인증 (1000 미인증, 1001 토큰무효, 1003 잘못된 op, 1004 JSON오류) |
| 2xxx | 채널 (2000 채널없음, 2001 정원초과, 2002 권한없음, 2003 이미참여, 2004 미참여) |
| 3xxx | 메시지 (3000 빈메시지, 3001 길이초과, 3002 미참여상태) |
| 9xxx | 서버 내부 (9000 알수없는에러) |

---

## 연결 흐름

```
클라이언트                           서버
    │                                 │
    │◄── op:0  HELLO ─────────────────│  heartbeat_interval 안내
    │─── op:3  IDENTIFY ─────────────►│  user_id, token, priority
    │◄── op:4  READY ─────────────────│  session_id 발급
    │                                 │
    │─── op:11 CHANNEL_JOIN ─────────►│  ssrc, ufrag, sdp_offer
    │◄── op:200 ACK ──────────────────│  sdp_answer, active_members
    │                                 │
    │    [ICE + DTLS 핸드셰이크 — UDP] │
    │◄══════════════════════════════► │
    │                                 │
    │─── op:30 FLOOR_REQUEST ────────►│  PTT 토글 ON
    │◄── op:110 FLOOR_GRANTED ────────│  발언권 허가 (본인)
    │◄── op:112 FLOOR_TAKEN ──────────│  발언 중 알림 (다른 멤버)
    │                                 │
    │─── op:32 FLOOR_PING ───────────►│  2초 주기 생존 신호
    │◄── op:116 FLOOR_PONG ───────────│  서버 응답
    │                                 │
    │─── op:31 FLOOR_RELEASE ────────►│  PTT 토글 OFF
    │◄── op:113 FLOOR_IDLE ───────────│  채널 유휴 (전체)
    │                                 │
    │─── op:12 CHANNEL_LEAVE ────────►│
    │◄── op:200 ACK ──────────────────│
    │─── [WS 종료] ───────────────────│  자동 클린업
```

---

## Floor Control (MBCP TS 24.380 기반)

채널별 발언권(Floor) 상태머신입니다.

```
G: Floor Idle
    │ FLOOR_REQUEST
    ▼
G: Floor Taken ──── FLOOR_RELEASE ──────────────► G: Floor Idle (또는 다음 Queue Grant)
    │
    ├── FLOOR_REQUEST (高 priority / Emergency) ──► Preempt → G: Floor Taken (신규 holder)
    ├── ping_timeout (6초 무응답) ────────────────► FLOOR_REVOKE → G: Floor Idle
    └── max_duration (30초 초과) ─────────────────► FLOOR_REVOKE → G: Floor Idle
```

### Ping/Pong 생존 확인

- holder가 `FLOOR_GRANTED` 수신 후 **2초 주기**로 `FLOOR_PING(op:32)` 자율 전송
- 서버는 수신 즉시 `FLOOR_PONG(op:116)` 응답
- 서버가 **6초** 이상 Ping을 못 받으면 `FLOOR_REVOKE(ping_timeout)` 발송

### 우선순위 (priority)

| 값 | 의미 |
|---|---|
| 255 | Emergency — priority 무관 즉시 Preempt |
| 200 | Imminent Peril |
| 100 | 일반 기본값 |

---

## 사전 생성 채널

서버 시작 시 아래 3개 채널이 자동으로 생성됩니다.

| channel_id | freq | name | 정원 |
|---|---|---|---|
| CH_0001 | 0001 | 📢 영업/시연 | 20 |
| CH_0002 | 0002 | 🤝 스스 파트너스 | 20 |
| CH_0003 | 0003 | 🏠 동천 패밀리 | 20 |

---

## lctrace — 실시간 시그널링 관찰 CLI

서버가 실행 중인 상태에서 시그널링 이벤트를 실시간으로 관찰합니다.  
SSE(Server-Sent Events)로 서버에 연결하며, 프로세스를 종료(Ctrl+C)하기 전까지 계속 수신합니다.

```bash
# 전체 이벤트 스트림
cargo run --bin lctrace

# 특정 채널만
cargo run --bin lctrace -- CH_0001

# Floor 이벤트만 (전체 채널)
cargo run --bin lctrace -- --filter floor

# 특정 채널 + Floor 이벤트만
cargo run --bin lctrace -- CH_0001 --filter floor

# 원격 서버
cargo run --bin lctrace -- --host 192.168.1.10 --port 8080 CH_0001
```

### 출력 예시

```
──────────────────────────────────────────────────────────────────────────────────────────
  lctrace ▶  http://127.0.0.1:8080/trace/CH_0001
──────────────────────────────────────────────────────────────────────────────────────────
  TIME           OP   DIR    OP_NAME                USER                CHANNEL   SUMMARY
──────────────────────────────────────────────────────────────────────────────────────────
  09:31:41.900    3 ↓ C→S  IDENTIFY               swift_falcon_4821   —         user=swift_falcon_4821
  09:31:42.010   11 ↓ C→S  CHANNEL_JOIN           swift_falcon_4821   —         user=swift_falcon_4821
  09:31:42.015  100 · SYS  CHANNEL_JOIN           swift_falcon_4821   CH_0001   user=swift_falcon_4821 ssrc=...
  09:31:42.100   30 ↓ C→S  FLOOR_REQUEST          swift_falcon_4821   CH_0001   user=swift_falcon_4821
  09:31:42.102  110 ↑ S→C  FLOOR_GRANTED          swift_falcon_4821   CH_0001   user=swift_falcon_4821 priority=100
  09:31:44.200   32 ↓ C→S  FLOOR_PING             swift_falcon_4821   CH_0001   user=swift_falcon_4821
  09:31:52.301   31 ↓ C→S  FLOOR_RELEASE          swift_falcon_4821   CH_0001   user=swift_falcon_4821
  09:31:52.303  113 ↑ S→C  FLOOR_RELEASE→IDLE     swift_falcon_4821   CH_0001   user=swift_falcon_4821
```

### 이벤트 색상

| 색상 | 의미 |
|---|---|
| 🟢 초록 bold | FLOOR_GRANTED |
| 🔴 빨간 bold | FLOOR_REVOKE, FLOOR_DENY |
| 🟡 노란색 | FLOOR_* (기타) |
| 🔵 청록색 | CHANNEL_JOIN, CHANNEL_LEAVE |
| 🟣 자주색 | IDENTIFY |

### SSE 엔드포인트 (직접 호출)

```bash
# curl로 직접 확인
curl -N http://127.0.0.1:8080/trace
curl -N http://127.0.0.1:8080/trace/CH_0001
```

---

## Admin REST API

`lcadmin` CLI가 내부적으로 사용하는 HTTP 엔드포인트입니다. `curl` 등으로 직접 호출도 가능합니다.

### 조회

| Method | Path | 설명 |
|---|---|---|
| GET | `/admin/status` | 서버 상태 요약 |
| GET | `/admin/users` | User 전체 목록 |
| GET | `/admin/users/{user_id}` | User 상세 |
| GET | `/admin/channels` | Channel 전체 목록 |
| GET | `/admin/channels/{channel_id}` | Channel 상세 |
| GET | `/admin/peers` | Endpoint 전체 목록 |
| GET | `/admin/peers/{ufrag}` | Endpoint 상세 |
| GET | `/channels` | 채널 목록 (일반) |
| GET | `/channels/{id}` | 채널 상세 (일반) |

### 조작

| Method | Path | 설명 |
|---|---|---|
| POST | `/admin/floor-revoke/{channel_id}` | Floor 강제 Idle 복귀 |

---

## 테스트

단위 테스트 71개가 각 모듈에 인라인(`#[cfg(test)]`)으로 작성되어 있습니다.

```bash
# 전체 테스트 (71개)
cargo test

# 특정 모듈만
cargo test core::floor
cargo test protocol::sdp
cargo test error
```

### 테스트 커버리지

| 모듈 | 테스트 수 | 검증 내용 |
|---|---|---|
| `core/user.rs` | 7 | register/unregister/count/duplicate/all_users/touch/zombie |
| `core/channel.rs` | 8 | create/duplicate/remove/add_member/capacity/dup_member/remove_member/floor_count |
| `core/floor.rs` | 14 | 상태전이/enqueue우선순위/중복/remove/position/preempt 3종/ping/timeout 3종 |
| `core/media_peer.rs` | 9 | insert/latch/remove/channel_filter/count/track_dedup/address/zombie |
| `error.rs` | 6 | 에러코드 범위 매핑 + display + 범위 검증 |
| `protocol/sdp.rs` | 14 | ice_string 3 + SDP answer 8 + BUNDLE 2 + detect_ip 1 |
| `trace.rs` | 4 | no_subscriber/subscribe/multi_subscriber/json직렬화 |
| `media/srtp.rs` | 5 | new/key_install/decrypt_before/encrypt_before/roundtrip |
| `media/net.rs` | 4 | classify_stun/dtls/srtp/unknown |

---

## 구현 현황

| 항목 | 상태 |
|---|---|
| WS 시그널링 프로토콜 | ✅ 완료 |
| 브로드캐스트 (채팅/이벤트) | ✅ 완료 |
| 상태 관리 3계층 | ✅ 완료 |
| IDENTIFY 토큰 검증 | ✅ 완료 |
| CLI 인자 (--port / --udp-port / --advertise-ip) | ✅ 완료 |
| SDP offer/answer 교환 (CHANNEL_JOIN) | ✅ 완료 |
| ICE Lite + STUN Binding | ✅ 완료 |
| DTLS 핸드셰이크 + keying material 추출 | ✅ 완료 |
| SRTP 암복호화 (webrtc-srtp) | ✅ 완료 |
| UDP 미디어 릴레이 + Floor 게이트 | ✅ 완료 |
| Floor Control (MBCP TS 24.380) | ✅ 완료 |
| 좀비 세션/피어 자동 종료 | ✅ 완료 |
| 사전 정의 채널 자동 생성 | ✅ 완료 |
| 운영 관리 CLI (lcadmin) | ✅ 완료 |
| 실시간 시그널링 관찰 CLI (lctrace) | ✅ 완료 |
| STUN keepalive 핫패스 최적화 | ✅ 완료 |
| 비디오 지원 (BUNDLE 확장) | ✅ 완료 |
| 모듈 분리 리팩터링 + 단위 테스트 71개 | ✅ 완료 |
| 배포 스크립트 (deploy-livechat.sh) | ✅ 완료 |
| net.rs SO_REUSEPORT + recvmmsg | 🔲 부하 테스트 후 적용 예정 |

---

## 라이선스

MIT
