use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use crate::resource::ResourceUsage;

/// 有界消息 ID 缓存，适用于长连接重放事件去重，例如 `msg-1 -> false -> true`。
pub(crate) struct BoundedIdCache {
    /// 最大缓存条目数。
    capacity: usize,
    /// 消息 ID 的插入顺序，用于淘汰最早条目。
    order: VecDeque<Arc<str>>,
    /// 已见消息 ID，用于常数时间查询。
    ids: HashSet<Arc<str>>,
}

impl BoundedIdCache {
    /// 创建有界缓存，适用于 channel 初始化，例如 `2048 -> 空缓存`。
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            order: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    /// 记录消息 ID 并返回是否已存在，例如首次 `msg-1 -> false`。
    pub(crate) fn seen_before(&mut self, message_id: &str) -> bool {
        if self.ids.contains(message_id) {
            return true;
        }
        let message_id = Arc::<str>::from(message_id);
        self.ids.insert(Arc::clone(&message_id));
        self.order.push_back(message_id);
        while self.order.len() > self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.ids.remove(oldest.as_ref());
            }
        }
        false
    }

    /// 清空去重窗口，适用于 channel reset，例如 `非空缓存 -> 空缓存`。
    pub(crate) fn clear(&mut self) {
        self.order.clear();
        self.ids.clear();
    }

    /// 返回缓存资源估算，适用于 resources 命令，例如 `channel.qq.dedup -> usage`。
    pub(crate) fn resource_usage(&self, name: &str) -> ResourceUsage {
        let order_bytes = self
            .order
            .capacity()
            .saturating_mul(std::mem::size_of::<Arc<str>>());
        let ids_bytes = self
            .ids
            .capacity()
            .saturating_mul(std::mem::size_of::<Arc<str>>());
        let content_bytes = self.order.iter().map(|item| item.len()).sum::<usize>();
        ResourceUsage::new(
            name,
            "cache",
            self.ids.len(),
            Some(self.capacity),
            order_bytes
                .saturating_add(ids_bytes)
                .saturating_add(content_bytes),
        )
    }
}
