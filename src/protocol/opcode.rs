// author: kodeholic (powered by Claude)

/// Client → Server opcodes
pub mod client {
    /// 클라이언트가 살아있음을 알림. d: 마지막 수신한 sequence 번호
    pub const HEARTBEAT:      u8 = 1;
    /// 연결 직후 인증 (user_id, token)
    pub const IDENTIFY:       u8 = 3;

    /// 채널 생성
    pub const CHANNEL_CREATE: u8 = 10;
    /// 채널 참여
    pub const CHANNEL_JOIN:   u8 = 11;
    /// 채널 나가기
    pub const CHANNEL_LEAVE:  u8 = 12;
    /// 채널 정보 수정
    pub const CHANNEL_UPDATE: u8 = 13;
    /// 채널 삭제
    pub const CHANNEL_DELETE: u8 = 14;
    /// 채널 목록 조회
    pub const CHANNEL_LIST:   u8 = 15;
    /// 채널 상세 조회 (채널 정보 + peer 목록)
    pub const CHANNEL_INFO:   u8 = 16;

    /// 채팅 메시지 전송
    pub const MESSAGE_CREATE: u8 = 20;

    // --- Floor Control (MBCP TS 24.380) ---
    /// PTT 누름 — 발언권 요청
    pub const FLOOR_REQUEST: u8 = 30;
    /// PTT 놓음 — 발언권 반납
    pub const FLOOR_RELEASE: u8 = 31;
    /// Floor Ping 응답 — 서버 Ping에 대한 생존 응답
    pub const FLOOR_PONG:    u8 = 32;
}

/// Server → Client opcodes
pub mod server {
    /// 연결 직후 서버가 heartbeat 주기를 알려줌
    pub const HELLO:           u8 = 0;
    /// HEARTBEAT 수신 확인
    pub const HEARTBEAT_ACK:   u8 = 2;
    /// IDENTIFY 성공. 세션 정보 전달
    pub const READY:           u8 = 4;

    /// 채널 내 멤버 변경 이벤트 브로드캐스트 (join/leave/update)
    pub const CHANNEL_EVENT:   u8 = 100;
    /// 채팅 메시지 브로드캐스트
    pub const MESSAGE_EVENT:   u8 = 101;

    /// 요청 성공 응답
    pub const ACK:             u8 = 200;
    /// 에러 응답
    pub const ERROR:           u8 = 201;

    // --- Floor Control (MBCP TS 24.380) ---
    /// 발언권 허가
    pub const FLOOR_GRANTED:        u8 = 110;
    /// 발언권 거부
    pub const FLOOR_DENY:           u8 = 111;
    /// 소속 채널에 누군가 발언 중 브로드캐스트
    pub const FLOOR_TAKEN:          u8 = 112;
    /// 채널이 유휴 상태임을 다운 안이운 동안 업데이트
    pub const FLOOR_IDLE:           u8 = 113;
    /// 발언권 강제 회수 (Preemption 또는 타임아웃)
    pub const FLOOR_REVOKE:         u8 = 114;
    /// 대기열 등록 확인 — Floor Deny 대신 대기열 진입 시 사용
    pub const FLOOR_QUEUE_POS_INFO: u8 = 115;
    /// 서버 → holder 생존 확인
    pub const FLOOR_PING:           u8 = 116;
}
