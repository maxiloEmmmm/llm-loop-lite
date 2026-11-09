use std::convert::Infallible;
use std::path::PathBuf;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use tower_http::services::{ServeDir, ServeFile};

use crate::channel::user_input::{PendingAnswer, PendingUserInputs};
use crate::error::{AppError, AppResult};
use crate::message::{InboundMessage, MessageSource};
use crate::query::{QueryError, QueryRequest, QueryService};

use super::WebInputMetadata;
use super::clients::ClientRegistry;
use super::protocol::{ClientEvent, ServerEvent};

/// Web HTTP 服务共享状态，适用于每条 WebSocket 连接复用 channel 队列和在线表。
#[derive(Clone)]
pub(super) struct WebServerState {
    /// channel 实例名。
    pub(super) channel_name: String,
    /// 可选连接认证令牌。
    pub(super) auth_token: Option<String>,
    /// 单条入站正文最大字节数。
    pub(super) max_message_bytes: usize,
    /// daemon 入站消息发送端。
    pub(super) inbound_tx: mpsc::Sender<InboundMessage>,
    /// 在线浏览器连接表。
    pub(super) clients: ClientRegistry,
    /// 等待中的结构化输入请求。
    pub(super) pending_inputs: std::sync::Arc<Mutex<PendingUserInputs<WebInputMetadata>>>,
    /// 无状态独立查询服务。
    pub(super) query_service: QueryService,
}

/// 健康检查响应，适用于状态页判断 Web 服务是否可用，例如 `GET /api/health -> ok`。
#[derive(Serialize)]
struct HealthResponse {
    /// 固定健康状态。
    status: &'static str,
    /// 当前 channel 名称。
    channel: String,
}

/// 启动 HTTP、WebSocket 与磁盘静态文件服务，例如 `listener + dist -> serving`。
pub(super) async fn serve(
    listener: TcpListener,
    state: WebServerState,
    static_dir: PathBuf,
    shutdown: CancellationToken,
) -> AppResult<()> {
    let index = static_dir.join("index.html");
    let static_files = ServeDir::new(static_dir).fallback(ServeFile::new(index));
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/query", post(query))
        .route("/api/ws", get(upgrade_websocket))
        .fallback_service(static_files)
        .with_state(state);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .map_err(|err| AppError::Channel(format!("web server failed: {err}")))
}

/// 执行无状态独立查询并返回 SSE，例如 `POST /api/query -> delta...done`。
async fn query(State(state): State<WebServerState>, Json(request): Json<QueryRequest>) -> Response {
    let receiver = match state.query_service.stream(request) {
        Ok(receiver) => receiver,
        Err(err) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(QueryError {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| {
            let event = Event::default()
                .json_data(chunk)
                .expect("StreamChunk serialization must succeed");
            (Ok::<Event, Infallible>(event), receiver)
        })
    });
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// 返回 Web channel 健康状态，例如 `GET /api/health -> {status: ok}`。
async fn health(State(state): State<WebServerState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        channel: state.channel_name,
    })
}

/// 升级浏览器 WebSocket，例如 `GET /api/ws -> 101`。
async fn upgrade_websocket(
    ws: WebSocketUpgrade,
    State(state): State<WebServerState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// 完成首帧认证并驱动单个浏览器连接，例如 `hello + message -> daemon inbound`。
async fn handle_socket(mut socket: WebSocket, state: WebServerState) {
    let hello = match tokio::time::timeout(Duration::from_secs(10), socket.recv()).await {
        Ok(Some(Ok(Message::Text(raw)))) => serde_json::from_str::<ClientEvent>(&raw),
        _ => return,
    };
    let ClientEvent::Hello {
        token,
        client_id,
        conversation_id,
    } = (match hello {
        Ok(event) => event,
        Err(_) => {
            let _ = send_socket_event(
                &mut socket,
                ServerEvent::Error {
                    message: "首条事件必须是合法 hello".to_string(),
                },
            )
            .await;
            return;
        }
    })
    else {
        let _ = send_socket_event(
            &mut socket,
            ServerEvent::Error {
                message: "首条事件必须是 hello".to_string(),
            },
        )
        .await;
        return;
    };
    if !valid_identifier(&client_id) || !valid_identifier(&conversation_id) {
        let _ = send_socket_event(
            &mut socket,
            ServerEvent::Error {
                message: "clientId 或 conversationId 不合法".to_string(),
            },
        )
        .await;
        return;
    }
    if state
        .auth_token
        .as_ref()
        .is_some_and(|expected| token.as_deref() != Some(expected.as_str()))
    {
        let _ = send_socket_event(
            &mut socket,
            ServerEvent::Error {
                message: "认证失败".to_string(),
            },
        )
        .await;
        return;
    }

    let (outbound_tx, mut outbound_rx) = mpsc::channel::<ServerEvent>(32);
    let connection_id = state.clients.register(&conversation_id, outbound_tx).await;
    if send_socket_event(
        &mut socket,
        ServerEvent::Ready {
            channel_name: state.channel_name.clone(),
            conversation_id: conversation_id.clone(),
        },
    )
    .await
    .is_err()
    {
        state.clients.remove(&conversation_id, &connection_id).await;
        return;
    }

    let source = MessageSource {
        channel_name: state.channel_name.clone(),
        platform: "web".to_string(),
        chat_id: conversation_id.clone(),
        chat_type: "dm".to_string(),
        user_id: Some(client_id),
        thread_id: None,
    };
    let (mut socket_sender, mut socket_receiver) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(event) = outbound_rx.recv().await {
            let Ok(raw) = serde_json::to_string(&event) else {
                continue;
            };
            if socket_sender.send(Message::Text(raw.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(frame) = socket_receiver.next().await {
        let Ok(Message::Text(raw)) = frame else {
            if matches!(frame, Ok(Message::Close(_))) {
                break;
            }
            continue;
        };
        if raw.len() > state.max_message_bytes.saturating_mul(2) {
            continue;
        }
        match serde_json::from_str::<ClientEvent>(&raw) {
            Ok(ClientEvent::Message { id, text }) => {
                let text = text.trim();
                if text.is_empty() || text.len() > state.max_message_bytes {
                    continue;
                }
                if state
                    .inbound_tx
                    .send(InboundMessage::text(text, source.clone(), Some(id)))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            Ok(ClientEvent::Answer {
                request_id,
                question_id,
                value,
            }) => {
                let mut pending = state.pending_inputs.lock().await;
                if let PendingAnswer::Accepted { finished, .. } =
                    pending.answer(&request_id, &question_id, value)
                    && finished
                {
                    pending.finish(&request_id);
                }
            }
            Ok(ClientEvent::Ping) => {
                state
                    .clients
                    .send(&conversation_id, ServerEvent::Pong)
                    .await;
            }
            Ok(ClientEvent::Hello { .. }) | Err(_) => {}
        }
    }
    state.clients.remove(&conversation_id, &connection_id).await;
    writer.abort();
}

/// 序列化并发送单条握手事件，例如 `ready -> WebSocket text frame`。
async fn send_socket_event(socket: &mut WebSocket, event: ServerEvent) -> AppResult<()> {
    let raw = serde_json::to_string(&event)?;
    socket
        .send(Message::Text(raw.into()))
        .await
        .map_err(|err| AppError::Channel(format!("websocket send failed: {err}")))
}

/// 校验浏览器生成 ID，避免超长或控制字符进入 session key，例如 `abc-123 -> true`。
fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}
