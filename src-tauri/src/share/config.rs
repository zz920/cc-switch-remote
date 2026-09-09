//! 组网（cc-switch-remote Share）常量与路径

use libp2p::StreamProtocol;
use std::path::PathBuf;

/// 消费侧本地桥接监听端口（仅 loopback）
pub const SHARE_BRIDGE_PORT: u16 = 15723;

/// Share P2P 监听端口（UDP/QUIC 与 TCP 共用端口号）。
///
/// 使用稳定端口便于 Windows 创建按程序和 Private 网络配置文件限定的
/// 防火墙规则；如端口冲突，可通过 CC_SWITCH_REMOTE_P2P_PORT 覆盖。
pub const SHARE_P2P_PORT: u16 = 15722;

pub fn share_p2p_port() -> u16 {
    std::env::var("CC_SWITCH_REMOTE_P2P_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(SHARE_P2P_PORT)
}

/// 数据面协议：每条 stream 承载一个 HTTP/1.1 请求/响应
pub const DATA_PLANE_PROTOCOL: StreamProtocol = StreamProtocol::new("/cc-switch-remote/http/1.0.0");

/// 加入审批协议（短码 + 二次确认）
pub const JOIN_PROTOCOL: StreamProtocol = StreamProtocol::new("/cc-switch-remote/join/1.0.0");

/// rendezvous 命名空间前缀（完整命名空间 = `cc-switch-remote:<share_id_hash>`）
pub const RENDEZVOUS_NAMESPACE_PREFIX: &str = "cc-switch-remote";

/// 官方 relay 地址（阿里云华北2；支持用户在设置中覆盖）
///
/// 格式：/ip4/<IP>/udp/15720/quic-v1/p2p/<PEER_ID> 或 /dns4/<域名>/tcp/15720/p2p/<PEER_ID>
/// QUIC 主选 + TCP 兜底；peer id 由 relay 首次启动生成的持久密钥决定，
/// 密钥丢失/更换服务器时需同步更新此处。
pub const DEFAULT_RELAY_ADDRS: &[&str] = &[
    "/dns4/tokentap.top/udp/15720/quic-v1/p2p/12D3KooWJviFfoWKwmhamp8Tsrx5GmDgGnoo3BTiAgqaLzRT7Qcz",
    "/dns4/tokentap.top/tcp/15720/p2p/12D3KooWJviFfoWKwmhamp8Tsrx5GmDgGnoo3BTiAgqaLzRT7Qcz",
];

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
pub const META_PATH: &str = "/__cc-switch-remote__/meta";

/// 消费侧指定出借方 Provider 的内部请求头。
/// 该头只在共享网络的 P2P 数据面中使用，forwarder 发往真实上游前会剥离。
pub const HEADER_ROUTE_PROVIDER: &str = "x-cc-switch-remote-share-provider";

/// 共享 Provider 连通性探测路径（不发起模型请求）。
pub const PROVIDER_CHECK_PATH: &str = "/__cc-switch-remote__/provider-check";

/// 组网数据目录（~/.cc-switch/share）
pub fn share_data_dir() -> PathBuf {
    let dir = crate::config::get_app_config_dir().join("share");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 未加入网络时也可使用的 relay 候选配置。
pub fn relay_config_path() -> PathBuf {
    share_data_dir().join("relay-addresses.txt")
}

/// 鉴权头名称
pub const HEADER_AUTH: &str = "x-cc-switch-remote-auth";
/// 组网头部前缀（出借侧出口净化时全部剥离）
pub const HEADER_PREFIX: &str = "x-cc-switch-remote-";
