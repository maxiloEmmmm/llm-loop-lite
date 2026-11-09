use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc};

use crate::resource::ResourceUsage;

use super::protocol::ServerEvent;

/// 在线浏览器连接表，按 conversation ID 广播回包，例如 `chat-1 -> 2 tabs`。
#[derive(Clone, Default)]
pub(super) struct ClientRegistry {
    /// 会话到连接发送端的映射。
    inner: Arc<Mutex<HashMap<String, HashMap<String, mpsc::Sender<ServerEvent>>>>>,
}

impl ClientRegistry {
    /// 注册浏览器连接并返回连接 ID，例如 `chat-1 + sender -> connection-id`。
    pub(super) async fn register(
        &self,
        conversation_id: &str,
        sender: mpsc::Sender<ServerEvent>,
    ) -> String {
        let connection_id = uuid::Uuid::new_v4().simple().to_string();
        self.inner
            .lock()
            .await
            .entry(conversation_id.to_string())
            .or_default()
            .insert(connection_id.clone(), sender);
        connection_id
    }

    /// 移除精确连接，避免旧连接断开时误删重连实例，例如 `chat-1 + conn-1 -> removed`。
    pub(super) async fn remove(&self, conversation_id: &str, connection_id: &str) {
        let mut inner = self.inner.lock().await;
        if let Some(connections) = inner.get_mut(conversation_id) {
            connections.remove(connection_id);
            if connections.is_empty() {
                inner.remove(conversation_id);
            }
        }
    }

    /// 向指定会话的所有页面发送事件，返回成功接收数量，例如 `chat-1 -> 2`。
    pub(super) async fn send(&self, conversation_id: &str, event: ServerEvent) -> usize {
        let connections = self
            .inner
            .lock()
            .await
            .get(conversation_id)
            .map(|connections| {
                connections
                    .iter()
                    .map(|(id, sender)| (id.clone(), sender.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut delivered = 0_usize;
        let mut closed = Vec::new();
        for (connection_id, sender) in connections {
            if sender.send(event.clone()).await.is_ok() {
                delivered += 1;
            } else {
                closed.push(connection_id);
            }
        }
        for connection_id in closed {
            self.remove(conversation_id, &connection_id).await;
        }
        delivered
    }

    /// 估算在线连接表内存，适用于 resources 命令，例如 `registry -> ResourceUsage`。
    pub(super) async fn resource_usage(&self, name: &str) -> ResourceUsage {
        let inner = self.inner.lock().await;
        let items = inner.values().map(HashMap::len).sum::<usize>();
        let estimated_bytes = inner
            .iter()
            .map(|(conversation_id, connections)| {
                conversation_id.capacity()
                    + connections.keys().map(String::capacity).sum::<usize>()
                    + connections
                        .capacity()
                        .saturating_mul(std::mem::size_of::<(String, mpsc::Sender<ServerEvent>)>())
            })
            .sum();
        ResourceUsage::new(name, "hashmap", items, None, estimated_bytes)
    }
}
