//! 飞书 WebSocket binary frame 的 protobuf 结构。

use prost::Message;

/// 飞书 frame header。
#[derive(Clone, PartialEq, Message)]
pub struct FeishuHeader {
    /// header key。
    #[prost(string, tag = "1")]
    pub key: String,
    /// header value。
    #[prost(string, tag = "2")]
    pub value: String,
}

/// 飞书 WebSocket frame。
#[derive(Clone, PartialEq, Message)]
pub struct FeishuFrame {
    /// 序列 id。
    #[prost(uint64, tag = "1")]
    pub seq_id: u64,
    /// 日志 id。
    #[prost(uint64, tag = "2")]
    pub log_id: u64,
    /// 服务 id。
    #[prost(int32, tag = "3")]
    pub service: i32,
    /// frame 类型：0 control，1 data。
    #[prost(int32, tag = "4")]
    pub method: i32,
    /// frame headers。
    #[prost(message, repeated, tag = "5")]
    pub headers: Vec<FeishuHeader>,
    /// payload 编码。
    #[prost(string, tag = "6")]
    pub payload_encoding: String,
    /// payload 类型。
    #[prost(string, tag = "7")]
    pub payload_type: String,
    /// payload 字节。
    #[prost(bytes, tag = "8")]
    pub payload: Vec<u8>,
    /// 新版日志 id。
    #[prost(string, tag = "9")]
    pub log_id_new: String,
}
