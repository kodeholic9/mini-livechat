// author: kodeholic (powered by Gemini)

// ============================================================
// 상태
// ============================================================
const state = {
  ws: null,
  pc: null,
  stream: null,
  audioSender: null, // RTCRtpSender 추적용
  analyser: null,
  meterTimer: null,
  hbTimer: null,
  ssrc: null,
  ufrag: null,
  channel: null,
  ready: false,
  pttActive: false,
  floorState: 'idle', // 'idle' | 'taken' | 'requesting' | 'queued'
  floorHolder: null, // 현재 발언 중인 user_id
  floorPingSeq: null, // 서버에서 받은 최신 ping seq
  userId: null, // 현재 로그인된 user_id 저장용
};

const $ = (id) => document.getElementById(id);

// ============================================================
// 오디오 송신 제어 (Privacy / Bandwidth Leak 방지)
// ============================================================
function setAudioTransmission(active) {
  if (!state.audioSender || !state.stream) return;
  // active가 true면 마이크 트랙 연결, false면 null로 교체하여 패킷 송신 원천 차단
  const track = active ? state.stream.getAudioTracks()[0] : null;
  state.audioSender
    .replaceTrack(track)
    .catch((e) => log('sys', `replaceTrack 에러: ${e.message}`, 'err'));
}

// ============================================================
// 로그
// ============================================================
function log(dir, msg, cls = '') {
  const now = new Date();
  const t = `${String(now.getMinutes()).padStart(2, '0')}:${String(now.getSeconds()).padStart(2, '0')}.${String(now.getMilliseconds()).padStart(3, '0')}`;
  const el = document.createElement('div');
  el.className = 'log-entry';
  el.innerHTML = `<span class="log-time">${t}</span><span class="log-dir ${dir}">${dir === 'in' ? '↓' : dir === 'out' ? '↑' : '·'}</span><span class="log-body ${cls}">${escHtml(msg)}</span>`;
  const area = $('log-area');
  area.appendChild(el);
  area.scrollTop = area.scrollHeight;
}

function escHtml(s) {
  return String(s)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
}

// ============================================================
// 상태 UI
// ============================================================
function setState(key, val, cls = '') {
  const el = $(`s-${key}`);
  if (!el) return;
  el.textContent = val;
  el.className = `state-val ${cls}`;
}

function setBadge(id, val, cls = '') {
  const el = $(id);
  if (!el) return;
  el.textContent = val;
  el.style.color =
    cls === 'ok'
      ? 'var(--accent)'
      : cls === 'err'
        ? 'var(--accent2)'
        : cls === 'warn'
          ? 'var(--warn)'
          : 'var(--text-dim)';
}

function randomSSRC() {
  return Math.floor(Math.random() * 0xffffffff) + 1;
}

// ============================================================
// WebSocket
// ============================================================
function wsConnect() {
  const url = $('srv-url').value.trim();
  log('sys', `connecting → ${url}`);
  state.ws = new WebSocket(url);

  state.ws.onopen = () => {
    $('dot').className = 'status-dot on';
    $('ws-status').textContent = 'CONNECTED';
    setState('ws', 'OPEN', 'ok');
    log('sys', 'WS connected', 'ok');
    setButtons('connected');
  };

  state.ws.onmessage = (e) => {
    let pkt;
    try {
      pkt = JSON.parse(e.data);
    } catch {
      log('in', e.data);
      return;
    }
    log('in', JSON.stringify(pkt));
    handlePacket(pkt);
  };

  state.ws.onclose = (e) => {
    $('dot').className = 'status-dot';
    $('ws-status').textContent = 'DISCONNECTED';
    setState('ws', 'CLOSED', 'err');
    log('sys', `WS closed (${e.code})`, 'err');
    clearInterval(state.hbTimer);
    setButtons('disconnected');
    state.ready = false;
  };

  state.ws.onerror = () => {
    $('dot').className = 'status-dot err';
    log('sys', 'WS error', 'err');
  };
}

function wsSend(obj) {
  if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
    log('sys', 'WS not open', 'err');
    return;
  }
  log('out', JSON.stringify(obj));
  state.ws.send(JSON.stringify(obj));
}

// ============================================================
// 패킷 핸들러
// ============================================================
function handlePacket(pkt) {
  switch (pkt.op) {
    case 0:
      onHello(pkt.d);
      break;
    case 2:
      /* HEARTBEAT_ACK */ break;
    case 4:
      onReady(pkt.d);
      break;
    case 100:
      onChannelEvent(pkt.d);
      break;
    case 101:
      onMessageEvent(pkt.d);
      break;
    case 200:
      onAck(pkt.d);
      break;
    case 201:
      onError(pkt.d);
      break;
    // Floor Control (MBCP TS 24.380)
    case 110:
      onFloorGranted(pkt.d);
      break;
    case 111:
      onFloorDeny(pkt.d);
      break;
    case 112:
      onFloorTaken(pkt.d);
      break;
    case 113:
      onFloorIdle(pkt.d);
      break;
    case 114:
      onFloorRevoke(pkt.d);
      break;
    case 115:
      onFloorQueuePos(pkt.d);
      break;
    case 116:
      onFloorPing(pkt.d);
      break;
  }
}

function onHello(d) {
  log('sys', `HELLO — heartbeat every ${d.heartbeat_interval}ms`);
  clearInterval(state.hbTimer);
  state.hbTimer = setInterval(() => {
    wsSend({ op: 1, d: null });
    const t = new Date()
      .toLocaleTimeString('ko-KR', { hour12: false })
      .slice(0, 5);
    setBadge('hb-badge', `HB: ${t}`);
  }, d.heartbeat_interval);
}

function onReady(d) {
  state.ready = true;
  state.userId = d.user_id;
  setState('user', d.user_id, 'ok');
  log('sys', `READY — session ${d.session_id}`, 'ok');
  setButtons('ready');
}

function onAck(d) {
  if (d.op === 11) onChannelJoinAck(d.data);
}

function onError(d) {
  log('sys', `ERROR ${d.code}: ${d.reason}`, 'err');
}

function onChannelEvent(d) {
  if (d.event === 'join') {
    log('sys', `${d.member.user_id} joined ${d.channel_id}`);
    addMember(d.member.user_id, d.member.ssrc);
  }
  if (d.event === 'leave') {
    log('sys', `${d.member.user_id} left ${d.channel_id}`);
    removeMember(d.member.user_id);
  }
}

function onMessageEvent(d) {
  log('in', `[${d.author_id}] ${d.content}`);
}

async function onChannelJoinAck(data) {
  setState('channel', data.channel_id, 'ok');
  state.channel = data.channel_id;
  log('sys', `joined ${data.channel_id}`, 'ok');

  $('member-list').innerHTML = '';
  (data.active_members || []).forEach((m) => addMember(m.user_id, m.ssrc));

  if (data.sdp_answer && state.pc) {
    $('sdp-viewer').textContent = data.sdp_answer;
    log('sys', 'SDP answer received — setting remote description...');
    try {
      await state.pc.setRemoteDescription({
        type: 'answer',
        sdp: data.sdp_answer,
      });
      log('sys', 'remote description set OK', 'ok');
      setState('dtls', 'NEGOTIATING', 'warn');
      setBadge('dtls-badge', 'DTLS: NEG', 'warn');
    } catch (e) {
      log('sys', `setRemoteDescription failed: ${e.message}`, 'err');
      setState('dtls', 'FAILED', 'err');
    }
  }

  setButtons('joined');
}

// ============================================================
// WebRTC
// ============================================================
async function setupWebRTC() {
  try {
    state.stream = await navigator.mediaDevices.getUserMedia({
      audio: true,
      video: false,
    });
    $('media-state').textContent = '마이크 획득';
    log('sys', 'microphone acquired', 'ok');
  } catch (e) {
    log('sys', `mic error: ${e.message}`, 'err');
    $('media-state').textContent = '마이크 실패 — SDP 없이 진행';
    return null;
  }

  state.pc = new RTCPeerConnection({
    iceServers: [],
    iceTransportPolicy: 'all',
  });

  // 트랙 추가 및 초기 송신 차단 (replaceTrack)
  const track = state.stream.getAudioTracks()[0];
  state.audioSender = state.pc.addTrack(track, state.stream);
  state.audioSender.replaceTrack(null);

  state.pc.onicecandidate = (e) => {
    if (e.candidate)
      log('sys', `ICE cand: ${e.candidate.candidate.substring(0, 70)}...`);
  };

  state.pc.oniceconnectionstatechange = () => {
    const s = state.pc.iceConnectionState;
    const cls =
      s === 'connected' || s === 'completed'
        ? 'ok'
        : s === 'failed'
          ? 'err'
          : 'warn';
    setState('ice', s.toUpperCase(), cls);
    setBadge('ice-badge', `ICE: ${s.substring(0, 4).toUpperCase()}`, cls);
    log('sys', `ICE state: ${s}`);
    // PTT 활성화는 DTLS 완료(connectionState) 이벤트에서만 처리
    // ICE connected != DTLS complete — 여기서 활성화하면 DTLS 전에 SRTP 송신됨
  };

  state.pc.onconnectionstatechange = () => {
    const s = state.pc.connectionState;
    const cls = s === 'connected' ? 'ok' : s === 'failed' ? 'err' : 'warn';
    setState('dtls', s.toUpperCase(), cls);
    setBadge('dtls-badge', `DTLS: ${s.substring(0, 4).toUpperCase()}`, cls);
    log('sys', `Connection state: ${s}`);

    // DTLS 핸드셰이크 완료 후에만 PTT 활성화
    // connectionState === 'connected' = ICE + DTLS 모두 완료
    if (s === 'connected') {
      setState('srtp', 'ACTIVE', 'ok');
      $('ptt-btn').disabled = false;
      $('media-state').textContent = 'DTLS 완료 — PTT 준비';
      startMeter();
      log('sys', 'DTLS handshake complete — PTT enabled', 'ok');
    } else if (s === 'failed') {
      setState('srtp', 'FAILED', 'err');
      $('media-state').textContent = 'DTLS 실패';
      log('sys', 'DTLS failed', 'err');
    }
  };

  state.pc.ontrack = (e) => {
    log('sys', 'remote audio track received', 'ok');
    const audio = new Audio();
    audio.srcObject = e.streams[0];
    audio.play().catch(() => {});
  };

  const offer = await state.pc.createOffer({ offerToReceiveAudio: true });
  await state.pc.setLocalDescription(offer);

  // ICE gathering 완료 대기
  await new Promise((resolve) => {
    if (state.pc.iceGatheringState === 'complete') {
      resolve();
      return;
    }
    state.pc.onicegatheringstatechange = () => {
      if (state.pc.iceGatheringState === 'complete') resolve();
    };
    setTimeout(resolve, 3000);
  });

  const sdpStr = state.pc.localDescription.sdp;
  const ssrcMatch = sdpStr.match(/a=ssrc:(\d+)/);
  state.ssrc = ssrcMatch ? parseInt(ssrcMatch[1]) : randomSSRC();

  const ufragMatch = sdpStr.match(/a=ice-ufrag:(\S+)/);
  state.ufrag = ufragMatch
    ? ufragMatch[1]
    : `uf${Math.random().toString(36).slice(2, 6)}`;

  setState('ssrc', state.ssrc);
  log('sys', `offer ready — ssrc=${state.ssrc} ufrag=${state.ufrag}`);

  return sdpStr;
}

// ============================================================
// 볼륨 미터
// ============================================================
function startMeter() {
  if (!state.stream || state.analyser) return;
  const ctx = new AudioContext();
  const src = ctx.createMediaStreamSource(state.stream);
  state.analyser = ctx.createAnalyser();
  state.analyser.fftSize = 256;
  src.connect(state.analyser);
  const data = new Uint8Array(state.analyser.frequencyBinCount);

  state.meterTimer = setInterval(() => {
    state.analyser.getByteFrequencyData(data);
    const avg = data.reduce((a, b) => a + b, 0) / data.length;
    const pct = Math.min(100, avg * 2.5);
    const fill = $('meter-fill');
    fill.style.width = `${pct}%`;
    fill.style.background = pct > 70 ? 'var(--accent2)' : 'var(--accent)';
  }, 50);
}

// ============================================================
// PTT + Floor Control (MBCP TS 24.380)
// ============================================================
function pttStart() {
  if (!state.stream || state.pttActive || !state.channel) return;
  state.pttActive = true;
  state.floorState = 'requesting';
  $('ptt-btn').classList.add('active');
  $('ptt-btn').textContent = '● REQUESTING…';
  wsSend({
    op: 30,
    d: {
      channel_id: state.channel,
      priority: parseInt($('priority').value, 10) || 100,
      indicator: 'normal',
    },
  });
}

function pttStop() {
  if (!state.pttActive) return;
  state.pttActive = false;
  if (state.floorState === 'taken' && state.floorHolder === state.userId) {
    wsSend({ op: 31, d: { channel_id: state.channel } });
  }
  if (state.floorState === 'requesting') {
    setAudioTransmission(false);
    _resetFloorUI();
  }
}

// ── Floor 이벤트 핸들러 ──
function onFloorGranted(d) {
  state.floorState = 'taken';
  state.floorHolder = d.user_id;
  state.userId = state.userId || d.user_id;
  setState('floor', 'TAKEN', 'ok');
  setState('holder', d.user_id, 'ok');
  setState('queue', '—');
  log(
    'sys',
    `FLOOR_GRANTED — ${d.user_id} (max ${(d.duration / 1000).toFixed(0)}s)`,
    'ok',
  );

  if (d.user_id === $('user-id').value.trim()) {
    setAudioTransmission(true); // 실제 패킷 전송 시작
    $('ptt-btn').textContent = '● TRANSMITTING';
    $('ptt-btn').classList.add('active');
  }
}

function onFloorDeny(d) {
  state.pttActive = false;
  state.floorState = 'idle';
  setState('floor', 'IDLE');
  setState('queue', '—');

  setAudioTransmission(false);
  $('ptt-btn').classList.remove('active');
  $('ptt-btn').textContent = '● PUSH TO TALK';
  log('sys', `FLOOR_DENY — ${d.reason}`, 'err');
}

function onFloorTaken(d) {
  state.floorHolder = d.user_id;
  if (d.user_id !== $('user-id').value.trim()) {
    state.floorState = 'taken';
    setState('floor', 'TAKEN', 'warn');
    setState('holder', d.user_id, 'warn');

    // 남이 발언 중일 때 UI
    $('ptt-btn').classList.remove('active');
    $('ptt-btn').textContent = '● IN USE (RX)';
  }
  _setMemberSpeaking(d.user_id, true);
  log('sys', `FLOOR_TAKEN — ${d.user_id} [${d.indicator}]`);
}

function onFloorIdle(d) {
  const prevHolder = state.floorHolder;
  const wasMine = prevHolder === $('user-id').value.trim();

  state.floorState = 'idle';
  state.floorHolder = null;
  setState('floor', 'IDLE');
  setState('holder', '—');
  setState('queue', '—');
  if (prevHolder) _setMemberSpeaking(prevHolder, false);

  if (wasMine) {
    setAudioTransmission(false);
    _resetFloorUI();
  } else {
    $('ptt-btn').textContent = '● PUSH TO TALK';
  }

  log('sys', `FLOOR_IDLE — ${d.channel_id}`);
}

function onFloorRevoke(d) {
  const wasMine = state.floorHolder === $('user-id').value.trim();
  state.pttActive = false;
  state.floorState = 'idle';

  setAudioTransmission(false);
  _resetFloorUI();

  if (wasMine) {
    log('sys', `FLOOR_REVOKE — cause: ${d.cause}`, 'err');
  }
}

function onFloorQueuePos(d) {
  state.floorState = 'queued';
  setState('floor', 'QUEUED', 'warn');
  setState('queue', `${d.queue_position}/${d.queue_size}`);
  $('ptt-btn').textContent = `● QUEUE ${d.queue_position}`;
  log('sys', `FLOOR_QUEUE — pos ${d.queue_position}/${d.queue_size}`);
}

function onFloorPing(d) {
  state.floorPingSeq = d.seq;
  wsSend({ op: 32, d: { channel_id: d.channel_id, seq: d.seq } });

  if (state.floorHolder === $('user-id').value.trim()) {
    if (state.audioSender && state.audioSender.track === null) {
      setAudioTransmission(true);
    }
  }
}

function _resetFloorUI() {
  const prev = state.floorHolder;
  if (prev) _setMemberSpeaking(prev, false);
  $('ptt-btn').classList.remove('active');
  $('ptt-btn').textContent = '● PUSH TO TALK';
  setState('floor', 'IDLE');
  setState('holder', '—');
  setState('queue', '—');
}

function _setMemberSpeaking(userId, isSpeaking) {
  const el = document.getElementById(`spk-${userId}`);
  if (!el) return;
  el.textContent = isSpeaking ? '▶ ON AIR' : '';
  el.style.color = 'var(--accent)';
  el.style.fontSize = '9px';
  el.style.fontFamily = 'var(--mono)';
}

// ============================================================
// 멤버 UI
// ============================================================
function addMember(userId, ssrc) {
  if (document.querySelector(`[data-uid="${userId}"]`)) return;
  const el = document.createElement('div');
  el.className = 'member-item';
  el.dataset.uid = userId;
  const initials =
    userId
      .replace(/[^a-zA-Z0-9]/g, '')
      .substring(0, 2)
      .toUpperCase() || '??';
  el.innerHTML = `
    <div class="member-avatar">${initials}</div>
    <div class="member-info">
      <div class="member-name">${escHtml(userId)}</div>
      <div class="member-ssrc">ssrc: ${ssrc || '—'}</div>
    </div>
    <div class="member-speaking" id="spk-${userId}"></div>`;
  $('member-list').appendChild(el);
}

function removeMember(userId) {
  document.querySelector(`[data-uid="${userId}"]`)?.remove();
}

// ============================================================
// 버튼 상태 및 이벤트
// ============================================================
function setButtons(mode) {
  const d = (id, v) => ($(id).disabled = v);
  if (mode === 'disconnected') {
    d('btn-connect', false);
    d('btn-disconnect', true);
    d('btn-identify', true);
    d('btn-create', true);
    d('btn-join', true);
    d('btn-leave', true);
    d('btn-send', true);
    $('chat-input').disabled = true;
    d('ptt-btn', true);
  } else if (mode === 'connected') {
    d('btn-connect', true);
    d('btn-disconnect', false);
    d('btn-identify', false);
  } else if (mode === 'ready') {
    d('btn-create', false);
    d('btn-join', false);
    d('ptt-btn', true);
    d('btn-leave', true);
    d('btn-send', true);
    $('chat-input').disabled = true;
  } else if (mode === 'joined') {
    d('btn-join', true);
    d('btn-leave', false);
    d('btn-send', false);
    $('chat-input').disabled = false;
    d('ptt-btn', false);
  }
}

$('btn-connect').onclick = () => wsConnect();

$('btn-disconnect').onclick = () => {
  clearInterval(state.hbTimer);
  clearInterval(state.meterTimer);
  state.analyser = null;
  state.pc?.close();
  state.pc = null;
  state.ws?.close();
  $('member-list').innerHTML = '';
  $('sdp-viewer').textContent = '—';
  $('media-state').textContent = '미디어 비활성';
  setState('ice', '—');
  setState('dtls', '—');
  setState('srtp', '—');
  setBadge('ice-badge', 'ICE: —');
  setBadge('dtls-badge', 'DTLS: —');
};

$('btn-identify').onclick = () => {
  wsSend({
    op: 3,
    d: {
      user_id: $('user-id').value.trim(),
      token: $('token').value.trim(),
      priority: parseInt($('priority').value, 10) || 100,
    },
  });
};

$('btn-create').onclick = () => {
  const ch = $('channel-id').value.trim();
  wsSend({ op: 10, d: { channel_id: ch, channel_name: ch } });
};

$('btn-join').onclick = async () => {
  const ch = $('channel-id').value.trim();
  if (!ch) {
    log('sys', 'channel_id 필요', 'err');
    return;
  }

  // 기존 PC 완전 정리 후 새로 생성 — 재JOIN 시 이전 DTLS 세션 잔재 제거
  if (state.pc) {
    state.pc.close();
    state.pc = null;
    state.audioSender = null;
    state.analyser = null;
    clearInterval(state.meterTimer);
    log('sys', 'previous PC closed', 'warn');
  }

  let sdpOffer = null;
  if (window.RTCPeerConnection) {
    sdpOffer = await setupWebRTC();
  }

  wsSend({
    op: 11,
    d: {
      channel_id: ch,
      ssrc: state.ssrc || randomSSRC(),
      ufrag: state.ufrag || `uf${Math.random().toString(36).slice(2, 6)}`,
      sdp_offer: sdpOffer,
    },
  });
};

$('btn-leave').onclick = () => {
  if (!state.channel) return;
  wsSend({ op: 12, d: { channel_id: state.channel } });
  state.channel = null;
  setState('channel', '—');
  $('member-list').innerHTML = '';
  $('sdp-viewer').textContent = '—';
  pttStop();
  state.floorState = 'idle';
  state.floorHolder = null;
  _resetFloorUI();
  state.pc?.close();
  state.pc = null;
  setButtons('ready');
};

function sendChat() {
  const txt = $('chat-input').value.trim();
  if (!txt || !state.channel) return;
  wsSend({ op: 20, d: { channel_id: state.channel, content: txt } });
  $('chat-input').value = '';
}
$('btn-send').onclick = sendChat;
$('chat-input').onkeydown = (e) => {
  if (e.key === 'Enter') sendChat();
};

$('btn-clear').onclick = () => {
  $('log-area').innerHTML = '';
};

// PTT 버튼 이벤트 처리
const pttBtn = $('ptt-btn');
pttBtn.onmousedown = () => pttStart();
pttBtn.onmouseup = () => pttStop();
pttBtn.onmouseleave = () => pttStop();
pttBtn.ontouchstart = (e) => {
  e.preventDefault();
  pttStart();
};
pttBtn.ontouchend = (e) => {
  e.preventDefault();
  pttStop();
};

document.addEventListener('keydown', (e) => {
  if (e.code === 'Space' && !e.repeat && !e.target.matches('input')) {
    e.preventDefault();
    pttStart();
  }
});
document.addEventListener('keyup', (e) => {
  if (e.code === 'Space') {
    e.preventDefault();
    pttStop();
  }
});

// 초기화
log('sys', 'mini-livechat E2E client ready');
log('sys', 'SPACEBAR = PTT while in channel');
