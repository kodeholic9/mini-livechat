# mini-livechat SDP 가이드 — 김대리 지침서

> **목적**: SFU(ice-lite, BUNDLE) 구조에서 `build_sdp_answer()` 구현 시
> 반복된 실수를 방지하고, 다음 대화에서 올바른 방향으로 코딩하기 위한 지침.
>
> **배경**: 종전 대화에서 SDP direction, BUNDLE, 포트, 코덱 미러링 등
> 여러 문제가 동시에 발생하며 디버깅이 꼬였음. 이 문서는 그 경험을 정제한 것.

---

## 1. 종전 대화에서 꼬인 원인 분석

### 1.1 문제가 겹쳐서 한 번에 터졌다

하나씩 보면 단순한데, 동시에 3~4개가 겹치니 원인 파악이 불가능했음.

| #   | 문제                        | 증상                                  | 근본 원인                                       |
| --- | --------------------------- | ------------------------------------- | ----------------------------------------------- |
| 1   | `a=group:BUNDLE` 누락       | 브라우저가 ICE candidate 매핑 실패    | 세션 헤더에 BUNDLE 그룹 미선언                  |
| 2   | `m=` 포트가 더미값 `9`      | 브라우저가 실제 서버 주소를 모름      | offer의 포트를 그대로 복사                      |
| 3   | `m=` payload type 불일치    | `setRemoteDescription` 파싱 에러      | m= 라인은 `111 0`인데 rtpmap은 8개 선언         |
| 4   | direction이 항상 `recvonly` | 브라우저가 서버→클라 미디어 수신 불가 | `sendrecv`/`sendonly` 구분 없이 일괄 `recvonly` |
| 5   | 빌드 미반영                 | 코드 수정했으나 서버에 적용 안 됨     | 0.30초 빌드 = 재컴파일 없었음 (파일 미저장)     |
| 6   | PC 미정리                   | 재접속 시 이전 DTLS 세션 간섭         | `pc.close()` 없이 새 PC 생성                    |

### 1.2 꼬인 순서

```
① direction 버그 발견 → recvonly를 sendrecv로 고치자
② 근데 빌드가 적용 안 됨 (파일 미저장) → 여전히 recvonly
③ "왜 안 되지?" → BUNDLE 누락, 포트 문제 등 다른 원인을 의심
④ 여러 곳을 동시에 수정 → 코덱 payload 불일치 발생
⑤ setRemoteDescription 파싱 에러 → 또 다른 문제로 보임
⑥ 점점 어디가 원인인지 알 수 없게 됨
```

**교훈: SDP 문제는 한 번에 하나씩만 고치고, 매번 빌드 확인 후 테스트.**

---

## 2. SDP Answer 조립 — 철칙

### 2.1 구조 원칙

```
SDP Answer = 세션 헤더 + N개의 미디어 섹션
```

```
v=0
o=mini-livechat {timestamp} {timestamp} IN IP4 {server_ip}
s=-
t=0 0
a=group:BUNDLE {mid0} {mid1} ...    ← ★ offer의 mid 목록 그대로
a=ice-lite

m=audio {server_port} UDP/TLS/RTP/SAVPF {offer의 payload types 그대로}
c=IN IP4 {server_ip}
a=ice-ufrag:{random}
a=ice-pwd:{random}
a=fingerprint:sha-256 {server_fingerprint}
a=setup:passive
a=rtcp-mux
a=rtcp-rsize
a=recvonly                            ← ★ 클라이언트 송신 트랙 → 서버는 recvonly
a=rtcp:9 IN IP4 0.0.0.0
a=mid:{offer의 mid 그대로}
{offer의 extmap, rtpmap, fmtp, rtcp-fb 라인 그대로 미러링}
                                      ★ a=msid, a=ssrc는 여기 없음!
                                      ★ recvonly면 DROP, sendonly면 서버가 새로 생성
a=candidate:1 1 udp 2113937151 {server_ip} {server_port} typ host generation 0
a=end-of-candidates

m=video {server_port} UDP/TLS/RTP/SAVPF {offer의 payload types 그대로}
... (audio와 동일 패턴)
```

### 2.2 반드시 지켜야 할 규칙 (체크리스트)

- [ ] **BUNDLE 그룹**: 세션 헤더에 `a=group:BUNDLE {모든 mid 공백 구분}` 필수
- [ ] **m= 포트**: offer의 `9`(더미)가 아닌 **서버 실제 UDP 포트**로 교체
- [ ] **m= payload types**: offer의 `m=` 라인에서 payload type 목록을 **그대로 복사**
  - `m=audio 10000 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126`
  - ↑ 이 숫자들과 아래 `a=rtpmap:` 라인들이 **1:1 대응**해야 함
- [ ] **코덱 라인 미러링**: `a=rtpmap`, `a=fmtp`, `a=rtcp-fb`, `a=extmap`, `a=mid`, `a=rtcp` → offer에서 그대로 복사
- [ ] **SSRC/MSID 처리** (⚠️ 절대 offer에서 복사 금지):
  - **recvonly m-line** (클라이언트 송신 트랙 수신): `a=ssrc`, `a=ssrc-group`, `a=msid` → **DROP** (서버가 보낼 데이터 없으므로 불필요. 넣으면 브라우저가 루프백으로 오해)
  - **sendonly m-line** (타인 미디어 하향 전송): 서버가 **자체 생성한 고유 SSRC/MSID를 새로 발급**하여 삽입 (예: `a=ssrc:{서버 생성값}`, `a=msid:server-{userId}-audio {trackId}`)
- [ ] **ICE/DTLS 라인**: offer 것 버리고 **서버 값으로 교체**
  - `a=ice-ufrag`, `a=ice-pwd`, `a=fingerprint`, `a=setup`, `a=candidate`, `c=`
- [ ] **c= 라인**: `c=IN IP4 {server_ip}` — `0.0.0.0` 쓰지 말 것
- [ ] **a=setup:passive**: 서버는 항상 passive (클라이언트가 DTLS client)
- [ ] **a=ice-lite**: 세션 헤더에 1회만
- [ ] **BUNDLE 시 ICE/DTLS 속성 위치**: BUNDLE에서는 첫 번째 m-line의 ICE/DTLS만 사용됨. 모든 m-line에 동일하게 넣어도 무방하나(안전한 방식), 두 번째 이후 m-line에 다른 값을 넣으면 **무시되거나 에러**. 일관성을 위해 모든 m-line에 동일 값 삽입 권장

### 2.3 Direction 규칙 — 가장 중요

```
┌─────────────────────────────────────────────────────────┐
│  클라이언트 offer direction  →  서버 answer direction    │
│  ─────────────────────────    ──────────────────────    │
│  sendrecv                  →  recvonly  (현재 PTT)      │
│  sendonly                  →  recvonly                   │
│  recvonly                  →  sendonly  (서버→클라 전송) │
│  inactive                  →  inactive                   │
└─────────────────────────────────────────────────────────┘
```

**현재 PTT 구조** (클라이언트가 SFU에 미디어 업로드):

- 클라이언트 offer: `sendrecv` 또는 `sendonly`
- 서버 answer: **`recvonly`** ← "나(서버)는 받기만 할게"
- 이건 현재 정상 동작 중

**컨퍼런스 확장 시** (SFU가 다른 참여자 미디어를 내려보낼 때):

- SFU re-offer의 새 m-line: **`sendonly`** ← "나(서버)가 보낼게"
- 클라이언트 answer: `recvonly`

**절대 하면 안 되는 것**:

- 모든 m-line에 일괄 `recvonly` 때리기 ← 종전 버그의 핵심 원인
- `sendrecv` 남발 ← 불필요한 양방향 개방, 의도 불명확

### 2.4 SSRC / MSID 규칙 — 독소 조항 방지 (⚠️ 치명적)

**offer의 `a=ssrc`, `a=ssrc-group`, `a=msid`를 answer에 그대로 복사(Echo)하면 안 된다.**

이유: SFU는 P2P가 아니다. 클라이언트가 "내 SSRC는 1234, MSID는 A-Audio"라고
보냈는데 서버가 그대로 돌려보내면, 브라우저는 "서버가 내 소리를 나한테 다시
쏘는 건가?" 하고 루프백으로 오해하여 트랙 처리를 멈추거나 demux 에러가 발생한다.

```
┌──────────────────────────────────────────────────────────────────┐
│  m-line direction     SSRC/MSID 처리                             │
│  ─────────────────    ──────────────────────────────────────     │
│  recvonly (서버 수신)  → DROP (서버가 보낼 데이터 없음)           │
│                         a=ssrc, a=msid 라인 자체를 생략           │
│                                                                  │
│  sendonly (서버 송신)  → 서버가 새로 생성한 고유값 삽입            │
│                         a=ssrc:{서버 생성 SSRC} cname:{고유값}    │
│                         a=msid:server-{userId}-audio {trackId}   │
│                         브라우저가 "이건 타인의 트랙" 으로 식별    │
└──────────────────────────────────────────────────────────────────┘
```

**요약**: 코덱 정보(`rtpmap`, `fmtp`, `extmap`)는 미러링. 신원 정보(`ssrc`, `msid`)는 절대 미러링 금지.

---

## 3. Offer 파싱 — 미러링 알고리즘

```
입력: offer SDP 문자열
출력: Vec<MediaSection> — 각 섹션별 { media_type, m_line, mid, codec_lines }

1. offer를 줄 단위로 순회
2. `m=` 만나면 새 MediaSection 시작
   - `m=audio` → media_type = Audio
   - `m=video` → media_type = Video
   - m_line = 이 줄 전체 (포트만 나중에 교체)
3. 미디어 섹션 내부 줄 분류:
   - SKIP (서버 값으로 교체하거나 버릴 것):
     a=ice-ufrag, a=ice-pwd, a=fingerprint, a=setup,
     a=candidate, a=end-of-candidates,
     a=sendrecv, a=sendonly, a=recvonly, a=inactive,
     a=rtcp-mux, a=rtcp-rsize, c=,
     a=ssrc, a=ssrc-group, a=msid          ← ⚠️ 절대 미러링 금지!
   - CAPTURE (미러링할 것):
     a=mid → mid 값 저장 + 라인 유지
     a=rtpmap, a=fmtp, a=rtcp-fb, a=extmap, a=rtcp → 그대로 복사
4. 다음 `m=` 만나면 이전 섹션 닫고 새 섹션 시작
5. EOF에서 마지막 섹션 닫기
```

### 3.1 m= 라인 포트 교체

```rust
// offer: "m=audio 9 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126"
// 변환: "m=audio 10000 UDP/TLS/RTP/SAVPF 111 63 9 0 8 13 110 126"
//                ^^^^^ 서버 포트로만 교체, 나머지 그대로!

fn replace_port(m_line: &str, port: u16) -> String {
    // "m=audio" 뒤 첫 번째 숫자만 교체
    let parts: Vec<&str> = m_line.splitn(3, ' ').collect();
    // parts[0] = "m=audio", parts[1] = "9", parts[2] = "UDP/TLS/RTP/SAVPF ..."
    format!("{} {} {}", parts[0], port, parts[2])
}
```

**절대 m= 라인을 직접 조립하지 말 것!**
`m=audio {port} UDP/TLS/RTP/SAVPF 111 0` ← 이렇게 하면 payload type 누락.

---

## 4. 컨퍼런스 확장 시 Re-negotiation 가이드

### 4.1 참여자 입장 시 SFU가 해야 할 일

```
새 참여자 C 입장:

1. C ↔ SFU: 초기 offer/answer (C의 송신 + A,B 수신용 m-line)
2. SFU → A: re-offer (기존 m-line 유지 + C의 audio/video m-line 추가)
3. SFU → B: re-offer (기존 m-line 유지 + C의 audio/video m-line 추가)
```

Re-offer에서 추가하는 sendonly m-line (⚠️ 2.4항 SSRC 규칙 준수):

```
m=audio {port} UDP/TLS/RTP/SAVPF {코덱 payload types}
c=IN IP4 {server_ip}
a=ice-ufrag:{기존 세션과 동일}       ← BUNDLE이므로 기존 ICE 세션 공유
a=ice-pwd:{기존 세션과 동일}
a=fingerprint:sha-256 {동일}
a=setup:passive
a=rtcp-mux
a=rtcp-rsize
a=sendonly                            ← SFU가 A에게 C의 미디어를 보냄
a=mid:{새 mid 번호}
a=msid:sfu-relay-userC-audio {SFU가 생성한 track-id}
a=ssrc:{SFU가 새로 생성한 SSRC} cname:sfu-{userC}
{코덱 라인: rtpmap, fmtp, extmap 등}
```

**⚠️ 주의**: 여기서 SSRC와 MSID는 **SFU가 자체 생성한 값**이다.
클라이언트 C의 offer에 있던 SSRC를 가져오는 게 아님!

- SFU가 C의 RTP 패킷을 A에게 릴레이할 때, **SFU가 SSRC를 rewrite**하거나
- transparent relay라면 **SDP의 a=ssrc 값과 실제 RTP 패킷의 SSRC가 일치**해야 함
- 어느 쪽이든 SDP에는 SFU가 관리하는 값을 넣어야 브라우저가 demux 가능

### 4.2 참여자 퇴장 시

```
m=audio 0 UDP/TLS/RTP/SAVPF ...    ← port=0으로 비활성화
a=inactive
```

또는:

```
기존 m-line 유지, direction만 a=inactive로 변경
```

**m-line은 SDP에서 삭제 불가** → inactive만 가능 → SDP bloat 주의

### 4.3 m-line 재활용 (SDP bloat 방지)

```
B 퇴장 → mid:2,3 inactive
D 입장 → mid:2,3을 D의 트랙으로 재활용 (direction을 sendonly로 복원)
```

---

## 5. 클라이언트 방어 코딩

### 5.1 setRemoteDescription 전 SDP 검증

```javascript
function validateSdpAnswer(sdp) {
  const errors = [];

  // 1. BUNDLE 그룹 존재 확인
  if (!sdp.includes("a=group:BUNDLE")) {
    errors.push("BUNDLE group missing");
  }

  // 2. m= 라인 개수와 BUNDLE mid 개수 일치 확인
  const mLines = sdp.match(/^m=/gm) || [];
  const bundleMatch = sdp.match(/a=group:BUNDLE (.+)/);
  if (bundleMatch) {
    const bundleMids = bundleMatch[1].trim().split(/\s+/);
    if (mLines.length !== bundleMids.length) {
      errors.push(
        `m-line count (${mLines.length}) != BUNDLE mid count (${bundleMids.length})`,
      );
    }
  }

  // 3. ice-lite 존재 확인 (SFU 필수)
  if (!sdp.includes("a=ice-lite")) {
    errors.push("ice-lite missing");
  }

  // 4. candidate 존재 확인
  if (!sdp.includes("a=candidate:")) {
    errors.push("no ICE candidate");
  }

  // 5. direction 확인
  if (!sdp.match(/a=(recvonly|sendonly|sendrecv|inactive)/)) {
    errors.push("no direction attribute");
  }

  // 6. payload type ↔ rtpmap 1:1 대응 확인 (종전 버그 재발 방지)
  const sections = sdp.split(/(?=^m=)/m);
  for (const section of sections) {
    const mMatch = section.match(/^m=\S+ \d+ \S+ (.+)/m);
    if (!mMatch) continue;
    const ptList = mMatch[1].trim().split(/\s+/);
    const rtpmapPts = [...section.matchAll(/^a=rtpmap:(\d+) /gm)].map(
      (m) => m[1],
    );
    for (const pt of ptList) {
      if (!rtpmapPts.includes(pt)) {
        errors.push(`payload type ${pt} in m-line but no matching a=rtpmap`);
      }
    }
  }

  return errors;
}
```

### 5.2 PC 생명주기 관리

```javascript
// 재접속 시 반드시 기존 PC 정리
async function reconnect() {
  if (state.pc) {
    state.pc.close();   // ← 이거 빠지면 이전 DTLS 세션이 간섭
    state.pc = null;
  }
  // 새 PC 생성
  state.pc = new RTCPeerConnection({ ... });
}
```

### 5.3 연결 상태 모니터링

```javascript
// ICE connected 안 되면 타임아웃 처리
let iceTimeout = setTimeout(() => {
  if (
    state.pc.iceConnectionState !== "connected" &&
    state.pc.iceConnectionState !== "completed"
  ) {
    log("sys", "ICE connection timeout — retrying...", "err");
    reconnect();
  }
}, 10000);

state.pc.oniceconnectionstatechange = () => {
  if (state.pc.iceConnectionState === "connected") {
    clearTimeout(iceTimeout);
  }
};
```

### 5.4 Re-negotiation 시 signalingState 체크

컨퍼런스에서 참여자가 빠르게 입퇴장하면, 이전 re-negotiation이 아직 진행 중인데
새 offer/answer가 들어올 수 있다. `signalingState`가 `stable`이 아닌 상태에서
`setRemoteDescription`이나 `createOffer`를 호출하면 `InvalidStateError` 발생.

```javascript
// re-offer 수신 시 signalingState 체크
async function handleReOffer(sdpOffer) {
  if (state.pc.signalingState !== "stable") {
    log(
      "sys",
      `signalingState=${state.pc.signalingState}, queuing re-offer`,
      "warn",
    );
    state.pendingOffer = sdpOffer; // 큐에 넣고 stable 될 때 처리
    return;
  }
  await applyReOffer(sdpOffer);
}

// stable 상태로 돌아오면 대기 중인 offer 처리
state.pc.onsignalingstatechange = () => {
  if (state.pc.signalingState === "stable" && state.pendingOffer) {
    const offer = state.pendingOffer;
    state.pendingOffer = null;
    handleReOffer(offer);
  }
};
```

### 5.5 Glare 처리 (동시 offer 충돌)

SFU가 re-offer를 보내는 동시에 클라이언트도 offer를 보내면 "glare" 충돌 발생.
ice-lite SFU 구조에서는 **SFU의 offer가 항상 우선** (polite/impolite 패턴).

```javascript
// Perfect Negotiation — 클라이언트는 "polite" 역할
async function handleReOffer(sdpOffer) {
  // glare 감지: 내가 offer를 보낸 상태에서 서버 offer가 왔다
  const isGlare = state.pc.signalingState === "have-local-offer";

  if (isGlare) {
    // 클라이언트(polite)가 양보: 내 offer를 rollback하고 서버 offer 수락
    log("sys", "glare detected — rolling back local offer", "warn");
    await state.pc.setLocalDescription({ type: "rollback" });
  }

  await state.pc.setRemoteDescription({ type: "offer", sdp: sdpOffer });
  const answer = await state.pc.createAnswer();
  await state.pc.setLocalDescription(answer);
  // answer를 SFU에 전송
  sendToSfu({ op: "sdp_answer", sdp: answer.sdp });
}
```

**원칙**: ice-lite SFU가 항상 "impolite"(우선권 보유), 클라이언트가 "polite"(양보).

---

## 6. 디버깅 체크리스트

문제 발생 시 이 순서대로 확인:

### 6.1 setRemoteDescription 실패 시

```
□ 서버 로그에서 실제 전송된 SDP answer 전문 확인
□ m= 라인의 payload type 목록과 a=rtpmap 라인이 1:1 대응하는가?
□ a=group:BUNDLE에 나열된 mid가 실제 m-line 수와 일치하는가?
□ 빌드 시간 확인 — 0.3초면 재컴파일 안 된 것 (파일 미저장 의심)
```

### 6.2 ICE failed 시

```
□ a=candidate에 서버 실제 IP/포트가 있는가? (0.0.0.0 아닌지)
□ c= 라인에 서버 실제 IP가 있는가?
□ 클라이언트 → 서버 UDP 포트 방화벽 열려있는가?
□ 기존 PC가 close() 없이 남아있지 않은가?
```

### 6.3 DTLS 성공 후 SRTP 실패 시

```
□ DTLS 완료 후 첫 SRTP 패킷까지 시간 간격 확인 (5초+ 이면 의심)
□ Replay protection window 설정 확인
□ direction 확인 — recvonly인데 서버가 보내려 하면 실패
```

### 6.4 비디오 안 나올 때

```
□ offer에 m=video 섹션이 있는가? (getUserMedia video:true 확인)
□ answer에 m=video 섹션이 있는가? (서버 미러링 확인)
□ answer의 video direction이 올바른가?
□ 모바일: autoplay policy — 사용자 제스처 후 play() 호출했는가?
```

### 6.5 Re-negotiation 실패 시

```
□ signalingState가 stable이었는가? (have-local-offer 상태에서 시도하면 에러)
□ Glare 충돌 — 서버 re-offer와 클라이언트 offer가 동시에 발생했는가?
□ 기존 m-line의 mid 번호가 유지되고 있는가? (mid 변경되면 브라우저 거부)
□ BUNDLE 그룹에 새 mid가 추가되었는가?
□ sendonly m-line에 SFU 생성 SSRC/MSID가 들어있는가? (offer echo 아닌지)
```

---

## 7. 코딩 시 김대리 행동 규칙

1. **SDP 관련 수정은 한 번에 하나만** — 동시에 여러 곳 고치면 원인 추적 불가
2. **m= 라인은 offer에서 복사** — 포트만 교체, payload type 직접 조립 금지
3. **direction은 offer 기준으로 반전** — sendrecv→recvonly, recvonly→sendonly
4. **SSRC/MSID는 절대 offer에서 복사하지 않는다** — recvonly면 DROP, sendonly면 서버가 새로 생성
5. **빌드 후 반드시 컴파일 시간 확인** — 1초 미만이면 미반영 의심
6. **서버 로그에 SDP 전문 출력** — 브라우저에 도달한 SDP와 비교 가능하게
7. **PC 재생성 시 반드시 close()** — 이전 DTLS 세션 간섭 방지
8. **검증 함수 먼저 실행** — setRemoteDescription 전에 validateSdpAnswer() 통과시키기
9. **re-negotiation 전 signalingState 확인** — stable 아니면 큐잉, glare면 rollback

---

_이 문서는 2026-02-27 ~ 2026-03-03 기간의 디버깅 경험을 정제한 것입니다._
_작성: 김대리 (kodeholic, powered by Claude)_
