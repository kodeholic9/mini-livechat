---
name: mini-livechat
description: |
  Rust + Tokio + Axum 기반 실시간 미디어 릴레이 서버 프로젝트(mini-livechat) 작업 컨텍스트.
  이 프로젝트에 대한 코딩, 설계 질문, 리팩터링, 테스트 작성이 언급되면 반드시 이 스킬을 사용할 것.
  "livechat", "livechat 서버", "미디어 릴레이", "srtp", "무전", "PTT", "채널 허브", "미디어 피어",
  "net.rs", "core.rs", "UserHub", "ChannelHub", "MediaPeerHub" 등의 키워드가 나오면 즉시 이 스킬을 참조할 것.
---

# Mini LiveChat — 작업 컨텍스트

## 프로젝트 개요

- **언어/프레임워크**: Rust + Tokio + Axum
- **목적**: 무전(PTT) 및 실시간 미디어 릴레이 백엔드 서버
- **로컬 경로**: `C:\work\github\mini-livechat`
- **현재 버전**: 0.20.0

---

## 소스 구조

```
src/
├── main.rs
├── lib.rs              — run_server(), mod 선언 (순수 오케스트레이션)
├── config.rs           — 전역 상수 (포트, 타임아웃, 정원, Floor 파라미터 등)
├── error.rs            — LiveError enum (1xxx~9xxx 에러 코드)
├── utils.rs            — current_timestamp() → u64 밀리초
├── reaper.rs           — 좀비 세션 자동 종료 태스크 (User/Endpoint/DTLS/Floor 타임아웃)
├── trace.rs            — TraceHub (broadcast 기반 이벤트 버스), TraceEvent, TraceDir
│
├── core.rs             — 서브모듈 선언 + re-export
└── core/
    ├── user.rs         — UserHub, User, BroadcastTx
    ├── channel.rs      — ChannelHub, Channel
    ├── floor.rs        — FloorControl, FloorControlState, FloorIndicator, FloorQueueEntry
    └── media_peer.rs   — MediaPeerHub, Endpoint, Track, TrackKind
│
├── protocol.rs         — 서브모듈 선언 + ADVERTISE_IP 전역
└── protocol/
    ├── opcode.rs       — client / server opcode 상수
    ├── message.rs      — GatewayPacket + payload 타입들 (C→S 12개, S→C 17개)
    ├── protocol.rs     — AppState, ws_handler, op 핸들러 12개, cleanup
    ├── sdp.rs          — build_sdp_answer(), detect_local_ip(), random_ice_string()
    └── floor.rs        — Floor Control 핸들러 (request/release/ping/timeout/disconnect)
│
├── media.rs            — pub use (진입점)
└── media/
    ├── net.rs          — UDP 수신 루프, RFC 7983 demux, STUN/DTLS/SRTP 핸들러, 릴레이
    ├── dtls.rs         — DTLS 핸드셰이크, ServerCert, DtlsSessionMap, UdpConnAdapter
    └── srtp.rs         — SrtpContext (Aes128CmHmacSha1_80 encrypt/decrypt)
│
├── http.rs             — 서브모듈 선언 + re-export
└── http/
    ├── state.rs        — HttpState
    ├── dto.rs          — 응답 DTO 17개
    ├── admin.rs        — Admin REST 핸들러 8개
    ├── channel.rs      — 일반 채널 조회 핸들러
    └── trace.rs        — Trace SSE 스트림 핸들러

bin/
├── admin.rs            — lcadmin CLI (운영 관리)
└── trace.rs            — lctrace CLI (실시간 시그널링 관찰)
```

---

## 핵심 자료구조

```rust
// IDENTIFY 시 등록되는 전역 라우팅 테이블
UserHub
    users: RwLock<HashMap<user_id, Arc<User>>>
        User { tx: BroadcastTx, last_seen: AtomicU64, priority: u8 }

// 채널 정의 + 멤버 목록 + Floor Control
ChannelHub
    channels: RwLock<HashMap<channel_id, Arc<Channel>>>
        Channel {
            channel_id, freq, name, capacity, created_at,
            members: RwLock<HashSet<user_id>>,
            floor:   Mutex<FloorControl>,
        }

// 미디어 릴레이 핫패스 — BUNDLE 구조 (피어당 1 Endpoint, 복수 Track)
MediaPeerHub
    by_ufrag: RwLock<HashMap<ufrag, Arc<Endpoint>>>    // ICE ufrag 주키 (불변)
    by_addr:  RwLock<HashMap<SocketAddr, Arc<Endpoint>>> // UDP 핫패스 캐시
        Endpoint {
            ufrag, ice_pwd, user_id, channel_id,
            address:       Mutex<Option<SocketAddr>>,  // STUN latching
            tracks:        RwLock<Vec<Track>>,          // ssrc + TrackKind
            last_seen:     AtomicU64,
            inbound_srtp:  Mutex<SrtpContext>,
            outbound_srtp: Mutex<SrtpContext>,
        }
```

---

## 프로토콜 (디스코드 스타일 opcode)

패킷 형식: `{ "op": N, "d": { ... } }`

### Client → Server
| op | 이름 |
|---|---|
| 1 | HEARTBEAT |
| 3 | IDENTIFY |
| 10 | CHANNEL_CREATE |
| 11 | CHANNEL_JOIN (ssrc + sdp_offer 포함) |
| 12 | CHANNEL_LEAVE |
| 13 | CHANNEL_UPDATE |
| 14 | CHANNEL_DELETE |
| 15 | CHANNEL_LIST |
| 16 | CHANNEL_INFO |
| 20 | MESSAGE_CREATE |
| 30 | FLOOR_REQUEST |
| 31 | FLOOR_RELEASE |
| 32 | FLOOR_PING |

### Server → Client
| op | 이름 |
|---|---|
| 0 | HELLO |
| 2 | HEARTBEAT_ACK |
| 4 | READY |
| 100 | CHANNEL_EVENT (join/leave/update/delete) |
| 101 | MESSAGE_EVENT |
| 200 | ACK |
| 201 | ERROR |
| 50 | FLOOR_GRANTED |
| 51 | FLOOR_DENY |
| 52 | FLOOR_REVOKE |
| 53 | FLOOR_TAKEN |
| 54 | FLOOR_IDLE |
| 55 | FLOOR_QUEUE_POS_INFO |
| 56 | FLOOR_PONG |

### 에러 코드 범위
| 범위 | 설명 |
|---|---|
| 1xxx | 연결/인증 |
| 2xxx | 채널 |
| 3xxx | 메시지 |
| 9xxx | 서버 내부 |

---

## 브로드캐스트 흐름

```
핸들러
    → ChannelHub.get(channel_id).get_members()      // HashSet<user_id>
    → UserHub.broadcast_to(members, json, exclude)  // mpsc tx.send()
```

## UDP 릴레이 흐름 (media/net.rs)

```
recv_from()
    → classify(packet)                    // RFC 7983 demux (STUN/DTLS/SRTP)
    → STUN: parse_username → latch → Binding Response (MESSAGE-INTEGRITY + FINGERPRINT)
    → DTLS: inject or start_handshake → export_keying_material → init_srtp_contexts
    → SRTP: by_addr O(1) → RTCP/RTP 분기 → decrypt → relay_to_channel
        → Floor Control 체크 → 같은 채널 다른 피어 encrypt → send_to
```

---

## Floor Control (MBCP TS 24.380 기반)

```
상태머신: Idle ←→ Taken

Request(Idle)  → Grant + FLOOR_TAKEN broadcast
Request(Taken) → can_preempt? → Preempt Revoke + Grant
               → else → Enqueue (priority 내림차순) + FLOOR_QUEUE_POS_INFO
Release        → clear_taken → dequeue_next? → Grant or Idle broadcast
Disconnect     → on_user_disconnect (holder면 Revoke, 대기열이면 remove)
Timeout        → reaper에서 주기 체크 (ping_timeout / max_duration → Revoke)
```

---

## 현재 구현 상태 및 다음 작업 순서

| 순서 | 항목 | 상태 |
|---|---|---|
| 1 | WS 시그널링 프로토콜 | ✅ 완료 |
| 2 | 3계층 상태 관리 (User/Channel/MediaPeer) | ✅ 완료 |
| 3 | UDP 릴레이 루프 + ICE Lite + STUN | ✅ 완료 |
| 4 | DTLS-SRTP 핸드셰이크 + 키 도출 | ✅ 완료 |
| 5 | SRTP 암복호화 + 미디어 릴레이 | ✅ 완료 |
| 6 | 좀비 세션 자동 종료 (Reaper) | ✅ 완료 |
| 7 | Floor Control (MBCP 기반 PTT) | ✅ 완료 |
| 8 | lcadmin / lctrace CLI 도구 | ✅ 완료 |
| 9 | 비디오 지원 (BUNDLE 확장) | ✅ 완료 |
| 10 | 리팩토링 (모듈 분리) + 단위 테스트 71개 | ✅ 완료 |
| 11 | 멀티 원격 비디오 | 🔲 다음 |
| 12 | E2E 비디오 테스트 | 🔲 예정 |

---

## 코딩 규칙

- 파일 상단 `// author: kodeholic (powered by Claude)` 명시
- 매직 넘버 금지 → `config.rs` 상수 사용
- `unwrap()` 남용 금지 → `LiveResult<T>` 또는 로그 후 `continue`
- 새 기능 추가 시 `CHANGELOG.md` 업데이트
- 코딩은 "코딩해줘" 명시적 요청 시에만 작성
- Rust 2018 edition 모듈 스타일 (`core.rs` + `core/`), `mod.rs` 미사용

---

## 자주 쓰는 명령

```bash
cargo build
cargo test
RUST_LOG=trace cargo run
```
