// author: kodeholic (powered by Claude)

use futures_util::{SinkExt, StreamExt};
use mini_livechat::core::{ChannelHub, MediaPeerHub, UserHub};
use mini_livechat::protocol::{ws_handler, AppState};
use portpicker::pick_unused_port;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

// ----------------------------------------------------------------------------
// [테스트 헬퍼]
// ----------------------------------------------------------------------------

async fn spawn_test_server() -> String {
    let port = pick_unused_port().expect("사용 가능한 포트를 찾을 수 없습니다.");
    let addr = format!("127.0.0.1:{}", port);

    let app_state = AppState {
        user_hub:       Arc::new(UserHub::new()),
        channel_hub:    Arc::new(ChannelHub::new()),
        media_peer_hub: Arc::new(MediaPeerHub::new()),
    };

    let app = axum::Router::new()
        .route("/ws", axum::routing::get(ws_handler))
        .with_state(app_state);

    let listener = TcpListener::bind(&addr).await.unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    addr
}

type WsTx = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;
type WsRx = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

async fn connect(addr: &str) -> (WsTx, WsRx) {
    let (ws, _) = connect_async(format!("ws://{}/ws", addr)).await.expect("WS 연결 실패");
    ws.split()
}

async fn send(tx: &mut WsTx, payload: Value) {
    tx.send(Message::Text(payload.to_string().into())).await.expect("전송 실패");
}

async fn recv(rx: &mut WsRx) -> Value {
    loop {
        match rx.next().await.expect("수신 실패").expect("메시지 에러") {
            Message::Text(t) => return serde_json::from_str(&t).expect("JSON 파싱 실패"),
            _ => continue,
        }
    }
}

fn assert_op(packet: &Value, expected_op: u64, label: &str) {
    assert_eq!(
        packet["op"].as_u64().unwrap(), expected_op,
        "{}: 기대 op={}, 실제={}", label, expected_op, packet["op"]
    );
}

/// HELLO → IDENTIFY → READY 까지 공통 처리
async fn identify(tx: &mut WsTx, rx: &mut WsRx, user_id: &str) {
    recv(rx).await; // HELLO
    send(tx, json!({ "op": 3, "d": { "user_id": user_id, "token": "tok" } })).await;
    recv(rx).await; // READY
}

/// 채널 생성 + 참여까지 공통 처리
async fn join_channel(tx: &mut WsTx, rx: &mut WsRx, channel_id: &str, ssrc: u32) {
    send(tx, json!({ "op": 10, "d": { "channel_id": channel_id, "channel_name": "테스트" } })).await;
    recv(rx).await; // CHANNEL_CREATE ACK
    send(tx, json!({ "op": 11, "d": { "channel_id": channel_id, "ssrc": ssrc } })).await;
    recv(rx).await; // CHANNEL_JOIN ACK
}

// ----------------------------------------------------------------------------
// [시나리오 1] HELLO → IDENTIFY → READY
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_identify_flow() {
    let addr = spawn_test_server().await;
    let (mut tx, mut rx) = connect(&addr).await;

    let hello = recv(&mut rx).await;
    assert_op(&hello, 0, "HELLO");
    assert!(hello["d"]["heartbeat_interval"].as_u64().unwrap() > 0);

    send(&mut tx, json!({ "op": 3, "d": { "user_id": "user_1", "token": "tok" } })).await;

    let ready = recv(&mut rx).await;
    assert_op(&ready, 4, "READY");
    assert_eq!(ready["d"]["user_id"], "user_1");
    assert!(ready["d"]["session_id"].as_str().unwrap().starts_with("sess_"));
}

// ----------------------------------------------------------------------------
// [시나리오 2] 인증 없이 요청 → ERROR 1000
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_unauthenticated_request() {
    let addr = spawn_test_server().await;
    let (mut tx, mut rx) = connect(&addr).await;

    recv(&mut rx).await; // HELLO
    send(&mut tx, json!({ "op": 11, "d": { "channel_id": "CH_1", "ssrc": 100 } })).await;

    let err = recv(&mut rx).await;
    assert_op(&err, 201, "ERROR");
    assert_eq!(err["d"]["code"], 1000, "NotAuthenticated 에러여야 합니다.");
}

// ----------------------------------------------------------------------------
// [시나리오 3] CHANNEL_CREATE → CHANNEL_JOIN → ACK
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_channel_create_and_join() {
    let addr = spawn_test_server().await;
    let (mut tx, mut rx) = connect(&addr).await;

    identify(&mut tx, &mut rx, "user_1").await;

    send(&mut tx, json!({ "op": 10, "d": { "channel_id": "CH_1", "channel_name": "테스트채널" } })).await;
    let ack = recv(&mut rx).await;
    assert_op(&ack, 200, "CHANNEL_CREATE ACK");
    assert_eq!(ack["d"]["op"], 10);

    send(&mut tx, json!({ "op": 11, "d": { "channel_id": "CH_1", "ssrc": 100 } })).await;
    let ack = recv(&mut rx).await;
    assert_op(&ack, 200, "CHANNEL_JOIN ACK");
    assert_eq!(ack["d"]["op"], 11);
    assert_eq!(ack["d"]["data"]["channel_id"], "CH_1");
}

// ----------------------------------------------------------------------------
// [시나리오 4] 브로드캐스트 — user_1 메시지 → user_2 수신
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_message_broadcast() {
    let addr = spawn_test_server().await;

    let (mut tx1, mut rx1) = connect(&addr).await;
    identify(&mut tx1, &mut rx1, "user_1").await;
    join_channel(&mut tx1, &mut rx1, "CH_CHAT", 101).await;

    let (mut tx2, mut rx2) = connect(&addr).await;
    identify(&mut tx2, &mut rx2, "user_2").await;

    // user_2는 채널 생성 없이 참여 (user_1이 이미 생성)
    send(&mut tx2, json!({ "op": 11, "d": { "channel_id": "CH_CHAT", "ssrc": 102 } })).await;
    recv(&mut rx2).await; // CHANNEL_JOIN ACK

    // user_1은 user_2 입장 이벤트 수신
    let join_event = recv(&mut rx1).await;
    assert_op(&join_event, 100, "CHANNEL_EVENT(join)");
    assert_eq!(join_event["d"]["event"], "join");
    assert_eq!(join_event["d"]["member"]["user_id"], "user_2");

    // user_1 메시지 전송
    send(&mut tx1, json!({ "op": 20, "d": { "channel_id": "CH_CHAT", "content": "안녕하세요!" } })).await;

    // 둘 다 MESSAGE_EVENT 수신
    let msg1 = recv(&mut rx1).await;
    let msg2 = recv(&mut rx2).await;

    assert_op(&msg1, 101, "MESSAGE_EVENT(user_1)");
    assert_op(&msg2, 101, "MESSAGE_EVENT(user_2)");
    assert_eq!(msg1["d"]["content"], "안녕하세요!");
    assert_eq!(msg2["d"]["content"], "안녕하세요!");
    assert_eq!(msg1["d"]["author_id"], "user_1");
}

// ----------------------------------------------------------------------------
// [시나리오 5] CHANNEL_LEAVE — 다른 멤버에게 leave 이벤트 전파
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_channel_leave_broadcast() {
    let addr = spawn_test_server().await;

    let (mut tx1, mut rx1) = connect(&addr).await;
    identify(&mut tx1, &mut rx1, "user_1").await;
    join_channel(&mut tx1, &mut rx1, "CH_L", 201).await;

    let (mut tx2, mut rx2) = connect(&addr).await;
    identify(&mut tx2, &mut rx2, "user_2").await;
    send(&mut tx2, json!({ "op": 11, "d": { "channel_id": "CH_L", "ssrc": 202 } })).await;
    recv(&mut rx2).await; // ACK

    recv(&mut rx1).await; // user_2 입장 이벤트 소비

    // user_2 퇴장
    send(&mut tx2, json!({ "op": 12, "d": { "channel_id": "CH_L" } })).await;
    recv(&mut rx2).await; // ACK

    // user_1이 leave 이벤트 수신
    let leave_event = recv(&mut rx1).await;
    assert_op(&leave_event, 100, "CHANNEL_EVENT(leave)");
    assert_eq!(leave_event["d"]["event"],              "leave");
    assert_eq!(leave_event["d"]["member"]["user_id"],  "user_2");
}

// ----------------------------------------------------------------------------
// [시나리오 6] 빈 메시지 → ERROR 3000
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_empty_message_error() {
    let addr = spawn_test_server().await;
    let (mut tx, mut rx) = connect(&addr).await;

    identify(&mut tx, &mut rx, "user_1").await;
    join_channel(&mut tx, &mut rx, "CH_E", 301).await;

    send(&mut tx, json!({ "op": 20, "d": { "channel_id": "CH_E", "content": "   " } })).await;

    let err = recv(&mut rx).await;
    assert_op(&err, 201, "ERROR");
    assert_eq!(err["d"]["code"], 3000, "EmptyMessage 에러여야 합니다.");
}

// ----------------------------------------------------------------------------
// [시나리오 7] HEARTBEAT → HEARTBEAT_ACK
// ----------------------------------------------------------------------------

#[tokio::test]
async fn test_heartbeat() {
    let addr = spawn_test_server().await;
    let (mut tx, mut rx) = connect(&addr).await;

    identify(&mut tx, &mut rx, "user_1").await;

    send(&mut tx, json!({ "op": 1, "d": null })).await;

    let ack = recv(&mut rx).await;
    assert_op(&ack, 2, "HEARTBEAT_ACK");
}
