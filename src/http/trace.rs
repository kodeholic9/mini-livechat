// author: kodeholic (powered by Claude)
// Trace SSE 스트림 핸들러
//
// GET /trace              — 전체 이벤트 스트림
// GET /trace/{channel_id} — 특정 채널 필터 후 스트림

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Sse},
};
use axum::response::sse::Event;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;

use super::state::HttpState;

pub async fn trace_stream(
    State(state): State<HttpState>,
    channel_filter: Option<Path<String>>,
) -> impl IntoResponse {
    let rx   = state.trace_hub.subscribe();
    let filter = channel_filter.map(|Path(id)| id);

    let stream = BroadcastStream::new(rx)
        .filter_map(move |result| {
            let f = filter.clone();
            match result {
                Err(_lagged) => None,
                Ok(event) => {
                    let pass = match &f {
                        None     => true,
                        Some(ch) => event.channel_id.as_deref() == Some(ch.as_str()),
                    };
                    if pass {
                        let json = serde_json::to_string(&event).unwrap_or_default();
                        Some(Ok::<Event, std::convert::Infallible>(
                            Event::default().data(json)
                        ))
                    } else {
                        None
                    }
                }
            }
        });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}
