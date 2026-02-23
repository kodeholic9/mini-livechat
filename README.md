# Mini LiveChat

초고성능 무전(PTT) 및 실시간 미디어 릴레이를 위한 경량 백엔드 서버 엔진입니다.
Rust + Tokio + Axum 기반으로 엣지 디바이스 환경에서도 안정적으로 동작하도록 설계되었습니다.

---

## 아키텍처 개요

```
클라이언트 (WS)
    │
    ▼
WebSocket Gateway (Axum)
    │
    ├── IDENTIFY     → UserHub 등록 (라우팅 테이블)
    ├── CHANNEL_JOIN → ChannelHub 멤버 등록 + MediaPeerHub SSRC 등록
    ├── MESSAGE      → ChannelHub 멤버 목록 → UserHub.broadcast_to()
    └── CHANNEL_LEAVE/WS 종료 → 자동 클린업

UDP (미디어 릴레이, net.rs — 구현 예정)
    │
    └── ssrc → MediaPeerHub O(1) 조회 → SRTP 암복호화 → 릴레이
```

### 상태 관리 3계층

| 허브 | 키 | 역할 |
|---|---|---|
| `UserHub` | user_id | WS 세션 + 브로드캐스트 라우팅 테이블 |
| `ChannelHub` | channel_id | 채널 정의 + 멤버 목록 |
| `MediaPeerHub` | ssrc | 미디어 릴레이 핫패스 (O(1) 조회) |

### 설계 원칙

- **제어/데이터 평면 분리** — WebSocket(시그널링)과 UDP(미디어)를 완전 분리
- **핫패스 O(1)** — SSRC 기반 MediaPeerHub로 UDP 패킷 수신 시 즉시 피어 조회
- **좀비 감지** — `last_seen` 기반 타임아웃으로 끊긴 세션/피어 자동 감지
- **Lock-Free 지향** — `AtomicU64`로 last_seen 갱신, `RwLock`으로 읽기 병렬화

---

## 프로토콜

디스코드 Gateway 스타일 opcode 기반 패킷 구조를 채택합니다.

```json
{ "op": 11, "d": { "channel_id": "CH_001", "ssrc": 12345 } }
```

### Client → Server Opcodes

| op | 이름 | 설명 |
|---|---|---|
| 1 | HEARTBEAT | 연결 유지 |
| 3 | IDENTIFY | 인증 (user_id, token) |
| 10 | CHANNEL_CREATE | 채널 생성 |
| 11 | CHANNEL_JOIN | 채널 참여 (ssrc 포함) |
| 12 | CHANNEL_LEAVE | 채널 나가기 |
| 13 | CHANNEL_UPDATE | 채널 정보 수정 |
| 14 | CHANNEL_DELETE | 채널 삭제 |
| 20 | MESSAGE_CREATE | 채팅 메시지 전송 |

### Server → Client Opcodes

| op | 이름 | 설명 |
|---|---|---|
| 0 | HELLO | 연결 직후 heartbeat 주기 안내 |
| 2 | HEARTBEAT_ACK | HEARTBEAT 수신 확인 |
| 4 | READY | IDENTIFY 성공, 세션 정보 전달 |
| 100 | CHANNEL_EVENT | 채널 멤버 변동 브로드캐스트 (join/leave/update/delete) |
| 101 | MESSAGE_EVENT | 채팅 메시지 브로드캐스트 |
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
클라이언트                       서버
    │                             │
    │◄── op:0 HELLO ──────────────│  heartbeat_interval 안내
    │                             │
    │─── op:3 IDENTIFY ──────────►│  user_id, token
    │◄── op:4 READY ──────────────│  session_id 발급
    │                             │
    │─── op:10 CHANNEL_CREATE ───►│
    │◄── op:200 ACK ──────────────│
    │                             │
    │─── op:11 CHANNEL_JOIN ─────►│  ssrc 포함
    │◄── op:200 ACK ──────────────│  active_members 포함
    │                             │
    │─── op:20 MESSAGE_CREATE ───►│
    │◄── op:101 MESSAGE_EVENT ────│  채널 전원 브로드캐스트
    │                             │
    │─── op:12 CHANNEL_LEAVE ────►│
    │◄── op:200 ACK ──────────────│
    │                             │
    │─── [WS 종료] ───────────────│  자동 클린업
```

---

## 빌드 및 실행

```bash
# 빌드
cargo build

# 서버 실행
RUST_LOG=info cargo run

# 트레이스 로깅과 함께 실행
RUST_LOG=trace cargo run

# 전체 테스트
cargo test

# 유닛 테스트만
cargo test --test core_test

# 통합 테스트만
cargo test --test integration_test
```

---

## 구현 현황

| 항목 | 상태 |
|---|---|
| WS 시그널링 프로토콜 | ✅ 완료 |
| 브로드캐스트 (채팅/이벤트) | ✅ 완료 |
| 상태 관리 3계층 | ✅ 완료 |
| UDP 미디어 릴레이 (RTP 평문) | ✅ 완료 |
| 좀비 세션 자동 종료 태스크 | ✅ 완료 |
| IDENTIFY 토큰 검증 (Secret Key) | ✅ 완료 |
| SRTP 암복호화 Phase 2 | 🔲 클라이언트 준비 후 예정 |

---

## 라이선스

MIT
