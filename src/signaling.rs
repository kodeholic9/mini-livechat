// author: kodeholic (powered by Gemini)

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, trace, warn};

use crate::core::{LiveChannelHub, LivePeerHub};

// ----------------------------------------------------------------------------
// [통신 규약] JSON 메시지 포맷
// ----------------------------------------------------------------------------

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClientMessage {
    Join {
        member_id: String,
        ssrc: u32,
        channel_id: String,
    },
    Leave {
        member_id: String,
    },
}

#[derive(Serialize, Debug, Clone)]
pub struct MemberInfo {
    pub member_id: String,
    pub ssrc: u32,
}

#[derive(Serialize, Debug)]
#[serde(tag = "type")]
pub enum ServerMessage {
    JoinSuccess {
        member_id: String,
        channel_id: String,
        active_members: Vec<MemberInfo>,
    },
    Error {
        code: u16,
        reason: String,
    },
}

// ----------------------------------------------------------------------------
// [웹소켓 핸들러 및 상태 관리]
// ----------------------------------------------------------------------------

/// Axum 웹 프레임워크와 우리 코어 엔진을 연결해 줄 공유 상태
#[derive(Clone)]
pub struct AppState {
    pub peer_hub: Arc<LivePeerHub>,
    pub channel_hub: Arc<LiveChannelHub>,
}

/// 클라이언트의 WS 연결 요청을 수락하고 소켓 루프를 Spawn 합니다.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// [핵심 로직] 개별 클라이언트의 웹소켓 생명주기를 담당하는 비동기 워커
async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut current_ssrc: Option<u32> = None;
    let mut current_channel: Option<String> = None;

    // 1. 패킷 수신 루프
    while let Some(msg) = socket.next().await {
        let msg = match msg {
            Ok(Message::Text(text)) => text,
            Ok(Message::Close(_)) => break, // 정상 종료 시 루프 탈출
            Err(e) => {
                warn!("웹소켓 에러 발생: {}", e);
                break;
            }
            _ => continue,
        };

        // 2. JSON 파싱
        let client_msg: ClientMessage = match serde_json::from_str(&msg) {
            Ok(parsed) => parsed,
            Err(e) => {
                warn!("잘못된 JSON 포맷 수신: {}", e);
                continue;
            }
        };

        // 3. 메시지 타입별 처리
        match client_msg {
            ClientMessage::Join { member_id, ssrc, channel_id } => {
                trace!("웹소켓 Join 요청 수신 - Member: {}, SSRC: {}", member_id, ssrc);

                // 코어 로직(LivePeerHub)에 유저 등록
                match state.peer_hub.join_channel(&member_id, ssrc, &channel_id, &state.channel_hub) {
                    Ok(_) => {
                        // 세션 상태 저장 (연결 끊김 시 정리를 위함)
                        current_ssrc = Some(ssrc);
                        current_channel = Some(channel_id.clone());

                        // 성공 응답 전송 (현재는 active_members를 빈 배열로 내려줌. 향후 채우기 구현 필요)
                        let response = ServerMessage::JoinSuccess {
                            member_id: member_id.clone(),
                            channel_id: channel_id.clone(),
                            active_members: vec![], 
                        };
                        
                        let _ = socket.send(Message::Text(serde_json::to_string(&response).unwrap().into())).await;
                    }
                    Err(e) => {
                        error!("Join 실패: {}", e);
                        let err_res = ServerMessage::Error { code: 403, reason: e.to_string() };
                        let _ = socket.send(Message::Text(serde_json::to_string(&err_res).unwrap().into())).await;
                    }
                }
            }
            ClientMessage::Leave { member_id } => {
                trace!("웹소켓 Leave 요청 수신 - Member: {}", member_id);
                break; // 루프를 탈출하면 하단의 '클린업 로직'이 자동으로 수행됨
            }
        }
    }

    // 4. [장애 대응] 클린업 로직 (웹소켓 연결 종료 시 반드시 수행)
    // 유저가 랜선을 뽑거나 브라우저를 닫아도 이 코드는 100% 실행되어 메모리 누수를 막습니다.
    if let (Some(ssrc), Some(channel_id)) = (current_ssrc, current_channel) {
        trace!("웹소켓 연결 종료됨. 코어 메모리 정리 시작 - SSRC: {}", ssrc);
        
        // 채널에서 유저 제거 (발언권 회수 포함)
        if let Some(channel) = state.channel_hub.channels.read().unwrap().get(&channel_id) {
            channel.remove_peer(ssrc);
        }
        
        // 전역 라우팅 테이블에서 제거 (메모리 완전 해제)
        state.peer_hub.peers.write().unwrap().remove(&ssrc);
        
        trace!("SSRC {} 데이터 클린업 완료", ssrc);
    }
}