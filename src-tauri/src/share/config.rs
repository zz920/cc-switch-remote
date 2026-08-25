//! 组网（TokenTap Share）常量与路径

use libp2p::StreamProtocol;
use std::path::PathBuf;

/// 消费侧本地桥接监听端口（仅 loopback）
pub const SHARE_BRIDGE_PORT: u16 = 15723;

/// 数据面协议：每条 stream 承载一个 HTTP/1.1 请求/响应
pub const DATA_PLANE_PROTOCOL: StreamProtocol = StreamProtocol::new("/tokentap/http/1.0.0");

/// 加入审批协议（短码 + 二次确认）
pub const JOIN_PROTOCOL: StreamProtocol = StreamProtocol::new("/tokentap/join/1.0.0");

/// rendezvous 命名空间前缀（完整命名空间 = `tokentap:<share_id_hash>`）
pub const RENDEZVOUS_NAMESPACE_PREFIX: &str = "tokentap";

/// 官方 relay 地址（部署后填入，见 docs/deploy；支持用户在设置中覆盖）
///
/// 格式：/ip4/<IP>/udp/15720/quic-v1/p2p/<PEER_ID> 或 /dns4/<域名>/tcp/15720/p2p/<PEER_ID>
pub const DEFAULT_RELAY_ADDRS: &[&str] = &[];

/// HMAC 时间戳窗口（±5 分钟）
pub const AUTH_WINDOW_SECS: i64 = 300;

/// 加入请求有效期（5 分钟未审批自动过期）
pub const JOIN_REQUEST_TTL_SECS: i64 = 300;

/// 防重放 nonce 缓存容量
pub const NONCE_CACHE_CAPACITY: usize = 10_000;

/// rendezvous 注册续期间隔（秒），注册 TTL 为 2h
pub const RENDEZVOUS_RENEW_SECS: u64 = 3600;

/// rendezvous 发现间隔（秒）
pub const RENDEZVOUS_DISCOVER_SECS: u64 = 30;

/// 能力通告（meta）查询间隔（秒）
pub const META_REFRESH_SECS: u64 = 60;

/// 出借侧能力查询路径
pub const META_PATH: &str = "/__tokentap__/meta";

/// 组网数据目录（~/.tokentap/share 或 ~/.cc-switch/share，迁移期兼容）
pub fn share_data_dir() -> PathBuf {
    let dir = crate::config::get_app_config_dir().join("share");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 鉴权头名称
pub const HEADER_AUTH: &str = "x-tokentap-auth";
/// 组网头部前缀（出借侧出口净化时全部剥离）
pub const HEADER_PREFIX: &str = "x-tokentap-";
