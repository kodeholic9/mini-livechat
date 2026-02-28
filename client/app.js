// author: kodeholic (powered by Claude)

// ============================================================
// 상태
// ============================================================
const state = {
  ws:           null,
  pc:           null,
  stream:       null,
  audioSender:  null,   // RTCRtpSender — 오디오
  videoSender:  null,   // RTCRtpSender — 비디오 (null = 비디오 미사용)
  videoEnabled: false,  // JOIN 전 사용자 설정값 (토글로 제어)
  analyser:     null,
  meterTimer:   null,
  hbTimer:      null,
  ssrc:         null,
  ufrag:        null,
  channel:      null,
  ready:        false,
  pttActive:    false,
  floorState:   'idle', // 'idle' | 'taken' | 'requesting' | 'queued'
  floorHolder:  null,
  floorPingTimer: null,
  userId:       null,
};

const $ = (id) => document.getElementById(id);

// ============================================================
// 미디어 송신 제어 — 오디오 + 비디오 동시
// active=true  : 트랙 연결 (패킷 전송 시작)
// active=false : null 대입 (패킷 원천 차단)
// ============================================================
function setMediaTransmission(active) {
  if (state.audioSender && state.stream) {
    const track = active ? (state.stream.getAudioTracks()[0] ?? null) : null;
    state.audioSender.replaceTrack(track)
      .catch((e) => log('sys', `audio replaceTrack: ${e.message}`, 'err'));
  }
  if (state.videoSender && state.stream) {
    const track = active ? (state.stream.getVideoTracks()[0] ?? null) : null;
    state.videoSender.replaceTrack(track)
      .catch((e) => log('sys', `video replaceTrack: ${e.message}`, 'err'));
  }
}

// 하위 호환 alias
function setAudioTransmission(active) { setMediaTransmission(active); }

// ============================================================
// 로그
// ============================================================
function log(dir, msg, cls = '') {
  const now = new Date();
  const t = `${String(now.getMinutes()).padStart(2,'0')}:${String(now.getSeconds()).padStart(2,'0')}.${String(now.getMilliseconds()).padStart(3,'0')}`;
  const el = document.createElement('div');
  el.className = 'log-entry';
  el.innerHTML = `<span class="log-time">${t}</span><span class="log-dir ${dir}">${dir==='in'?'↓':dir==='out'?'↑':'·'}</span><span class="log-body ${cls}">${escHtml(msg)}</span>`;
  const area = $('log-area');
  area.appendChild(el);
  area.scrollTop = area.scrollHeight;
}

function escHtml(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
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
  el.style.color = cls==='ok' ? 'var(--accent)' : cls==='err' ? 'var(--accent2)' : cls==='warn' ? 'var(--warn)' : 'var(--text-dim)';
}

function randomSSRC() {
  return Math.floor(Math.random() * 0xffffffff) + 1;
}

// ============================================================
// 비디오 토글 UI
// ============================================================
function initVideoToggle() {
  const checkbox = $('video-toggle');
  const track    = $('video-toggle-track');
  const thumb    = $('video-toggle-thumb');
  const label    = $('video-toggle-label');
  const section  = $('video-section');

  function applyToggle(on) {
    state.videoEnabled = on;
    track.style.background = on ? 'var(--accent)' : 'var(--muted)';
    thumb.style.transform  = on ? 'translateX(16px)' : 'translateX(0)';
    label.textContent      = on ? 'ON' : 'OFF';
    label.style.color      = on ? 'var(--accent)' : 'var(--text-dim)';
    section.style.display  = on ? 'block' : 'none';

    // 이미 JOIN 중이고 비디오를 켰다면 경고
    if (on && state.channel) {
      log('sys', '비디오 설정은 다음 JOIN 시 적용됩니다', 'warn');
    }
  }

  checkbox.addEventListener('change', () => applyToggle(checkbox.checked));
  // 초기 상태 적용
  applyToggle(false);
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
    try { pkt = JSON.parse(e.data); } catch { log('in', e.data); return; }
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
    case 0:   onHello(pkt.d);         break;
    case 2:   /* HEARTBEAT_ACK */     break;
    case 4:   onReady(pkt.d);         break;
    case 100: onChannelEvent(pkt.d);  break;
    case 101: onMessageEvent(pkt.d);  break;
    case 200: onAck(pkt.d);           break;
    case 201: onError(pkt.d);         break;
    case 110: onFloorGranted(pkt.d);  break;
    case 111: onFloorDeny(pkt.d);     break;
    case 112: onFloorTaken(pkt.d);    break;
    case 113: onFloorIdle(pkt.d);     break;
    case 114: onFloorRevoke(pkt.d);   break;
    case 115: onFloorQueuePos(pkt.d); break;
    case 116: onFloorPong(pkt.d);     break;
  }
}

function onHello(d) {
  log('sys', `HELLO — heartbeat every ${d.heartbeat_interval}ms`);
  clearInterval(state.hbTimer);
  state.hbTimer = setInterval(() => {
    wsSend({ op: 1, d: null });
    const t = new Date().toLocaleTimeString('ko-KR', { hour12: false }).slice(0, 5);
    setBadge('hb-badge', `HB: ${t}`);
  }, d.heartbeat_interval);
}

function onReady(d) {
  state.ready  = true;
  state.userId = d.user_id;
  setState('user', d.user_id, 'ok');
  log('sys', `READY — session ${d.session_id}`, 'ok');
  setButtons('ready');
}

function onAck(d) {
  if (d.op === 11) onChannelJoinAck(d.data);
  if (d.op === 15) onChannelListAck(d.data);
}

function onChannelListAck(list) {
  const sel  = $('channel-select');
  const prev = sel.value;
  sel.innerHTML = '<option value="">— 채널을 선택하세요 —</option>';
  (list || []).forEach((ch) => {
    const opt = document.createElement('option');
    opt.value = ch.channel_id;
    opt.textContent = `${ch.freq}  ${ch.name}  [${ch.member_count}/${ch.capacity}]`;
    sel.appendChild(opt);
  });
  if (prev && [...sel.options].some((o) => o.value === prev)) {
    sel.value = prev;
  } else {
    const def = [...sel.options].find((o) => o.value === 'CH_0001');
    if (def) sel.value = 'CH_0001';
  }
  log('sys', `CHANNEL_LIST 수신 (${(list || []).length}개)`, 'ok');
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
      await state.pc.setRemoteDescription({ type: 'answer', sdp: data.sdp_answer });
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
  // 1. getUserMedia — 비디오 토글에 따라 요청
  try {
    state.stream = await navigator.mediaDevices.getUserMedia({
      audio: true,
      video: state.videoEnabled
        ? { width: { ideal: 640 }, height: { ideal: 480 }, frameRate: { ideal: 15 } }
        : false,
    });
    const hasVideo = state.stream.getVideoTracks().length > 0;
    $('media-state').textContent = hasVideo ? '마이크 + 카메라 획득' : '마이크 획득';
    log('sys', `media acquired — audio:true video:${hasVideo}`, 'ok');

    // 로컬 비디오 프리뷰
    if (hasVideo) {
      $('local-video').srcObject = state.stream;
    }
  } catch (e) {
    log('sys', `getUserMedia error: ${e.message}`, 'err');
    $('media-state').textContent = '미디어 획득 실패 — SDP 없이 진행';
    return null;
  }

  // 2. PeerConnection 생성
  state.pc = new RTCPeerConnection({ iceServers: [], iceTransportPolicy: 'all' });

  // 3. 트랙 추가 — 초기에는 null로 차단, FLOOR_GRANTED 시 연결
  const audioTrack = state.stream.getAudioTracks()[0];
  state.audioSender = state.pc.addTrack(audioTrack, state.stream);
  state.audioSender.replaceTrack(null);

  const videoTrack = state.stream.getVideoTracks()[0];
  if (videoTrack) {
    state.videoSender = state.pc.addTrack(videoTrack, state.stream);
    state.videoSender.replaceTrack(null);
    log('sys', 'video track added to PC (muted until GRANTED)');
  } else {
    state.videoSender = null;
  }

  // 4. ICE 이벤트
  state.pc.onicecandidate = (e) => {
    if (e.candidate) log('sys', `ICE cand: ${e.candidate.candidate.substring(0, 70)}...`);
  };

  state.pc.oniceconnectionstatechange = () => {
    const s   = state.pc.iceConnectionState;
    const cls = s === 'connected' || s === 'completed' ? 'ok' : s === 'failed' ? 'err' : 'warn';
    setState('ice', s.toUpperCase(), cls);
    setBadge('ice-badge', `ICE: ${s.substring(0, 4).toUpperCase()}`, cls);
    log('sys', `ICE state: ${s}`);
  };

  // 5. DTLS 완료 감지
  state.pc.onconnectionstatechange = () => {
    const s   = state.pc.connectionState;
    const cls = s === 'connected' ? 'ok' : s === 'failed' ? 'err' : 'warn';
    setState('dtls', s.toUpperCase(), cls);
    setBadge('dtls-badge', `DTLS: ${s.substring(0, 4).toUpperCase()}`, cls);
    log('sys', `Connection state: ${s}`);

    if (s === 'connected') {
      setState('srtp', 'ACTIVE', 'ok');
      $('ptt-btn').disabled = false;
      const videoReady = state.videoSender ? ' + VIDEO' : '';
      $('media-state').textContent = `DTLS 완료 — PTT 준비${videoReady}`;
      startMeter();
      log('sys', `DTLS complete — PTT enabled${videoReady}`, 'ok');
    } else if (s === 'failed') {
      setState('srtp', 'FAILED', 'err');
      $('media-state').textContent = 'DTLS 실패';
      log('sys', 'DTLS failed', 'err');
    }
  };

  // 6. 수신 트랙 처리
  state.pc.ontrack = (e) => {
    const kind = e.track.kind;
    log('sys', `remote ${kind} track received`, 'ok');

    if (kind === 'audio') {
      const audio = new Audio();
      audio.srcObject = e.streams[0];
      audio.play().catch(() => {});
    } else if (kind === 'video') {
      // Floor holder의 비디오를 remote-video 엘리먼트에 표시
      $('remote-video').srcObject = e.streams[0];
    }
  };

  // 7. Offer 생성
  const offer = await state.pc.createOffer({
    offerToReceiveAudio: true,
    offerToReceiveVideo: state.videoEnabled,
  });
  await state.pc.setLocalDescription(offer);

  // ICE gathering 완료 대기
  await new Promise((resolve) => {
    if (state.pc.iceGatheringState === 'complete') { resolve(); return; }
    state.pc.onicegatheringstatechange = () => {
      if (state.pc.iceGatheringState === 'complete') resolve();
    };
    setTimeout(resolve, 3000);
  });

  const sdpStr = state.pc.localDescription.sdp;
  const ssrcMatch  = sdpStr.match(/a=ssrc:(\d+)/);
  state.ssrc = ssrcMatch ? parseInt(ssrcMatch[1]) : randomSSRC();
  const ufragMatch = sdpStr.match(/a=ice-ufrag:(\S+)/);
  state.ufrag = ufragMatch ? ufragMatch[1] : `uf${Math.random().toString(36).slice(2, 6)}`;

  setState('ssrc', state.ssrc);
  log('sys', `offer ready — ssrc=${state.ssrc} ufrag=${state.ufrag} video=${state.videoEnabled}`);
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
    fill.style.width      = `${pct}%`;
    fill.style.background = pct > 70 ? 'var(--accent2)' : 'var(--accent)';
  }, 50);
}

// ============================================================
// PTT + Floor Control (MBCP TS 24.380)
// ============================================================
function pttStart() {
  if (!state.stream || state.pttActive || !state.channel) return;
  state.pttActive  = true;
  state.floorState = 'requesting';
  $('ptt-btn').classList.add('active');
  $('ptt-btn').textContent = '● REQUESTING…';
  wsSend({ op: 30, d: { channel_id: state.channel, priority: parseInt($('priority').value, 10) || 100, indicator: 'normal' } });
}

function pttStop() {
  if (!state.pttActive) return;
  state.pttActive = false;

  if (state.floorState === 'taken' && state.floorHolder === state.userId) {
    _stopFloorPing();
    wsSend({ op: 31, d: { channel_id: state.channel } });

  } else if (state.floorState === 'requesting') {
    wsSend({ op: 31, d: { channel_id: state.channel } });
    setMediaTransmission(false);
    _resetFloorUI();

  } else if (state.floorState === 'queued') {
    wsSend({ op: 31, d: { channel_id: state.channel } });
    setMediaTransmission(false);
    _resetFloorUI();

  } else {
    setMediaTransmission(false);
    _resetFloorUI();
  }
}

// ── Floor 이벤트 핸들러 ──

function onFloorGranted(d) {
  state.floorState  = 'taken';
  state.floorHolder = d.user_id;
  state.userId      = state.userId || d.user_id;
  setState('floor',  'TAKEN', 'ok');
  setState('holder', d.user_id, 'ok');
  setState('queue',  '—');
  log('sys', `FLOOR_GRANTED — ${d.user_id} (max ${(d.duration/1000).toFixed(0)}s)`, 'ok');

  if (d.user_id === $('user-id').value.trim()) {
    setMediaTransmission(true);  // 오디오 + 비디오 동시 전송 시작
    const v = state.videoSender ? ' + VIDEO' : '';
    $('ptt-btn').textContent = `● TRANSMITTING${v}`;
    $('ptt-btn').classList.add('active');
    _startFloorPing(d.channel_id);
  }
}

function onFloorDeny(d) {
  state.pttActive  = false;
  state.floorState = 'idle';
  setState('floor', 'IDLE');
  setState('queue', '—');
  setMediaTransmission(false);
  $('ptt-btn').classList.remove('active');
  $('ptt-btn').textContent = '● PUSH TO TALK';
  log('sys', `FLOOR_DENY — ${d.reason}`, 'err');
}

function onFloorTaken(d) {
  state.floorHolder = d.user_id;
  if (d.user_id !== $('user-id').value.trim()) {
    state.floorState = 'taken';
    setState('floor',  'TAKEN', 'warn');
    setState('holder', d.user_id, 'warn');
    $('ptt-btn').classList.remove('active');
    $('ptt-btn').textContent = '● IN USE (RX)';
  }
  _setMemberSpeaking(d.user_id, true);
  log('sys', `FLOOR_TAKEN — ${d.user_id} [${d.indicator}]`);
}

function onFloorIdle(d) {
  const prevHolder = state.floorHolder;
  const myId       = $('user-id').value.trim();
  const wasMine    = prevHolder === myId;
  const wasQueued  = state.floorState === 'queued'; // 상태 변경 전에 캐시

  state.floorState  = 'idle';
  state.floorHolder = null;
  if (prevHolder) _setMemberSpeaking(prevHolder, false);

  // 리모트 비디오 클리어
  $('remote-video').srcObject = null;

  if (wasMine) {
    _stopFloorPing();
    setMediaTransmission(false);
    state.pttActive = false;
    _resetFloorUI();
  } else if (wasQueued || state.pttActive) {
    state.pttActive = false;
    _resetFloorUI();
  } else {
    _resetFloorUI();
  }

  log('sys', `FLOOR_IDLE — ${d.channel_id}`);
}

function onFloorRevoke(d) {
  const wasMine = state.floorHolder === $('user-id').value.trim();

  state.pttActive   = false;
  state.floorState  = 'idle';
  state.floorHolder = null;
  $('remote-video').srcObject = null;

  if (wasMine) {
    _stopFloorPing();
    setMediaTransmission(false);
    log('sys', `FLOOR_REVOKE — cause: ${d.cause}`, 'err');
  } else {
    setMediaTransmission(false);
    log('sys', `FLOOR_REVOKE — cause: ${d.cause} (queue 취소)`);
  }
  _resetFloorUI();
}

function onFloorQueuePos(d) {
  state.floorState = 'queued';
  setState('floor', 'QUEUED', 'warn');
  setState('queue', `${d.queue_position}/${d.queue_size}`);
  $('ptt-btn').textContent = `● QUEUE ${d.queue_position}`;
  log('sys', `FLOOR_QUEUE — pos ${d.queue_position}/${d.queue_size}`);
}

function onFloorPong(_d) { /* 로깅 용도만 — 현재 UI 변화 없음 */ }

function _startFloorPing(channelId) {
  _stopFloorPing();
  state.floorPingTimer = setInterval(() => {
    if (!state.channel) { _stopFloorPing(); return; }
    wsSend({ op: 32, d: { channel_id: channelId } });
  }, 2000);
}

function _stopFloorPing() {
  if (state.floorPingTimer) { clearInterval(state.floorPingTimer); state.floorPingTimer = null; }
}

function _resetFloorUI() {
  $('ptt-btn').classList.remove('active');
  $('ptt-btn').textContent = '● PUSH TO TALK';
  setState('floor',  'IDLE');
  setState('holder', '—');
  setState('queue',  '—');
}

function _setMemberSpeaking(userId, isSpeaking) {
  const el = document.getElementById(`spk-${userId}`);
  if (!el) return;
  el.textContent   = isSpeaking ? '▶ ON AIR' : '';
  el.style.color   = 'var(--accent)';
  el.style.fontSize  = '9px';
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
  const initials = userId.replace(/[^a-zA-Z0-9]/g,'').substring(0,2).toUpperCase() || '??';
  el.innerHTML = `
    <div class="member-avatar">${initials}</div>
    <div class="member-info">
      <div class="member-name">${escHtml(userId)}</div>
      <div class="member-ssrc">ssrc: ${ssrc||'—'}</div>
    </div>
    <div class="member-speaking" id="spk-${userId}"></div>`;
  $('member-list').appendChild(el);
}

function removeMember(userId) {
  document.querySelector(`[data-uid="${userId}"]`)?.remove();
}

// ============================================================
// 버튼 상태
// ============================================================
function setButtons(mode) {
  const d = (id, v) => { const el = $(id); if (el) el.disabled = v; };
  if (mode === 'disconnected') {
    d('btn-connect', false); d('btn-disconnect', true); d('btn-identify', true);
    d('btn-create', true);   d('btn-refresh', true);    d('btn-join', true);
    d('btn-leave', true);    d('btn-send', true);        d('ptt-btn', true);
    $('chat-input').disabled   = true;
    $('channel-select').disabled = true;
  } else if (mode === 'connected') {
    d('btn-connect', true); d('btn-disconnect', false); d('btn-identify', false);
  } else if (mode === 'ready') {
    d('btn-create', false); d('btn-refresh', false); d('btn-join', false);
    d('ptt-btn', true);     d('btn-leave', true);     d('btn-send', true);
    $('chat-input').disabled   = true;
    $('channel-select').disabled = false;
    wsSend({ op: 15, d: null });
  } else if (mode === 'joined') {
    d('btn-join', true); d('btn-leave', false); d('btn-send', false);
    $('chat-input').disabled = false;
    // PTT는 DTLS 완료(onconnectionstatechange) 후 활성화
  }
}

// ============================================================
// 이벤트 바인딩
// ============================================================
$('btn-connect').onclick = () => wsConnect();

$('btn-disconnect').onclick = () => {
  clearInterval(state.hbTimer);
  clearInterval(state.meterTimer);
  state.analyser = null;
  state.pc?.close(); state.pc = null;
  state.ws?.close();
  $('member-list').innerHTML  = '';
  $('sdp-viewer').textContent = '—';
  $('media-state').textContent = '미디어 비활성';
  $('local-video').srcObject  = null;
  $('remote-video').srcObject = null;
  setState('ice', '—'); setState('dtls', '—'); setState('srtp', '—');
  setBadge('ice-badge', 'ICE: —'); setBadge('dtls-badge', 'DTLS: —');
};

$('btn-identify').onclick = () => {
  wsSend({ op: 3, d: { user_id: $('user-id').value.trim(), token: $('token').value.trim(), priority: parseInt($('priority').value, 10) || 100 } });
};

$('btn-refresh').onclick = () => wsSend({ op: 15, d: null });

$('btn-create').onclick = () => {
  const freq = $('ch-freq').value.trim();
  const name = $('ch-name').value.trim();
  const id   = $('ch-id').value.trim();
  if (!freq || !name || !id) { log('sys', '주파수 / 채널명 / channel_id 필수', 'err'); return; }
  wsSend({ op: 10, d: { channel_id: id, freq, channel_name: name } });
  setTimeout(() => wsSend({ op: 15, d: null }), 200);
};

$('btn-join').onclick = async () => {
  const ch = $('channel-select').value;
  if (!ch) { log('sys', '채널을 선택하세요', 'err'); return; }

  if (state.pc) {
    state.pc.close(); state.pc = null;
    state.audioSender = null; state.videoSender = null;
    state.analyser = null;
    clearInterval(state.meterTimer);
    $('local-video').srcObject  = null;
    $('remote-video').srcObject = null;
    log('sys', 'previous PC closed', 'warn');
  }

  let sdpOffer = null;
  if (window.RTCPeerConnection) sdpOffer = await setupWebRTC();

  wsSend({ op: 11, d: { channel_id: ch, ssrc: state.ssrc || randomSSRC(), ufrag: state.ufrag || `uf${Math.random().toString(36).slice(2,6)}`, sdp_offer: sdpOffer } });
};

$('btn-leave').onclick = () => {
  if (!state.channel) return;
  wsSend({ op: 12, d: { channel_id: state.channel } });
  state.channel = null;
  setState('channel', '—');
  $('member-list').innerHTML = '';
  $('sdp-viewer').textContent = '—';
  $('local-video').srcObject  = null;
  $('remote-video').srcObject = null;
  _stopFloorPing();
  pttStop();
  state.floorState  = 'idle';
  state.floorHolder = null;
  _resetFloorUI();
  state.pc?.close(); state.pc = null;
  setButtons('ready');
  setTimeout(() => wsSend({ op: 15, d: null }), 200);
};

function sendChat() {
  const txt = $('chat-input').value.trim();
  if (!txt || !state.channel) return;
  wsSend({ op: 20, d: { channel_id: state.channel, content: txt } });
  $('chat-input').value = '';
}
$('btn-send').onclick = sendChat;
$('chat-input').onkeydown = (e) => { if (e.key === 'Enter') sendChat(); };
$('btn-clear').onclick    = () => { $('log-area').innerHTML = ''; };

// PTT 토글
const pttBtn = $('ptt-btn');
pttBtn.onclick     = () => { if (state.pttActive) pttStop(); else pttStart(); };
pttBtn.ontouchstart = (e) => { e.preventDefault(); if (state.pttActive) pttStop(); else pttStart(); };

document.addEventListener('keydown', (e) => {
  if (e.code === 'Space' && !e.repeat && !e.target.matches('input')) {
    e.preventDefault();
    if (state.pttActive) pttStop(); else pttStart();
  }
});

// ============================================================
// 랜덤 User ID 생성
// ============================================================
const ADJ  = ['swift','brave','silent','sharp','iron','ghost','storm','cold','dark','bold','echo','lone'];
const NOUN = ['falcon','wolf','raven','viper','eagle','shark','hawk','lynx','fox','cobra','tiger','bear'];

function generateUserId() {
  const rand = (arr) => arr[Math.floor(crypto.getRandomValues(new Uint32Array(1))[0] / 0xFFFFFFFF * arr.length)];
  const num  = String(crypto.getRandomValues(new Uint16Array(1))[0]).padStart(4,'0').slice(-4);
  return `${rand(ADJ)}_${rand(NOUN)}_${num}`;
}

$('btn-gen-id').onclick = () => { $('user-id').value = generateUserId(); };

// ============================================================
// 초기화
// ============================================================
initVideoToggle();
$('user-id').value = generateUserId();
log('sys', 'mini-livechat E2E client ready');
log('sys', 'SPACEBAR = PTT  |  VIDEO 토글 → JOIN 전에 설정');
