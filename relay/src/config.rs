//! 服务器运行配置：全部通过环境变量覆盖，默认值面向小型自建场景。

use std::{
    env,
    net::IpAddr,
    path::PathBuf,
    time::Duration,
};

use tracing::warn;

/// 默认监听端口（QUIC/UDP 与 TCP 共用同一端口号）
pub const DEFAULT_PORT: u16 = 15720;
/// 默认密钥文件路径（相对工作目录）
pub const DEFAULT_KEY_PATH: &str = "relay.key";

/// 中继资源限制（对应 `libp2p::relay::Config` 的各项上限）
///
/// 调参原则：relay 只应作为 DCUtR 打洞失败时的兜底路径，
/// 上限宁可保守，避免服务器被当成免费无限带宽滥用。
#[derive(Debug, Clone)]
pub struct RelayLimits {
    /// 全服最大同时存活的中继预约数（一个预约 = 一个客户端挂在 relay 上可被拨达）
    pub max_reservations: usize,
    /// 单个 Peer 最大预约数
    pub max_reservations_per_peer: usize,
    /// 预约有效期（到期未续约自动释放）
    pub reservation_duration: Duration,
    /// 全服最大同时存活的中继电路数（一条电路 = 一对客户端之间的一条中继连接）
    pub max_circuits: usize,
    /// 单个 Peer 作为端点的最大电路数
    pub max_circuits_per_peer: usize,
    /// 单条电路最长存活时间（硬上限，到点强制关闭）
    pub max_circuit_duration: Duration,
    /// 单条电路最大传输字节数（硬上限，耗尽强制关闭）
    ///
    /// 与 `max_circuit_duration` 共同决定单连接平均带宽上限：
    /// 平均带宽 ≈ max_circuit_bytes / max_circuit_duration
    /// 默认 1 GiB / 10 min ≈ 1.75 MB/s
    pub max_circuit_bytes: u64,
}

impl Default for RelayLimits {
    fn default() -> Self {
        Self {
            max_reservations: 256,
            max_reservations_per_peer: 2,
            reservation_duration: Duration::from_secs(60 * 60),
            max_circuits: 128,
            max_circuits_per_peer: 8,
            max_circuit_duration: Duration::from_secs(10 * 60),
            max_circuit_bytes: 1 << 30,
        }
    }
}

impl From<&RelayLimits> for libp2p::relay::Config {
    fn from(limits: &RelayLimits) -> Self {
        // 以 libp2p 默认配置为底（`..Default::default()` 保留其内置的按 Peer/IP
        // 速率限制器——外部 crate 无法自行构造 RateLimiter，只能在默认基础上覆盖字段）
        libp2p::relay::Config {
            max_reservations: limits.max_reservations,
            max_reservations_per_peer: limits.max_reservations_per_peer,
            reservation_duration: limits.reservation_duration,
            max_circuits: limits.max_circuits,
            max_circuits_per_peer: limits.max_circuits_per_peer,
            max_circuit_duration: limits.max_circuit_duration,
            max_circuit_bytes: limits.max_circuit_bytes,
            ..Default::default()
        }
    }
}

/// 服务器总体配置
#[derive(Debug, Clone)]
pub struct Config {
    /// 监听端口
    pub port: u16,
    /// Ed25519 密钥持久化文件路径
    pub key_path: PathBuf,
    /// 公网 IP（可选）：设置后作为 external address 通告给预约的客户端
    pub public_ip: Option<IpAddr>,
    /// 中继资源限制
    pub limits: RelayLimits,
}

impl Config {
    /// 从环境变量加载配置，未设置的项使用默认值
    pub fn from_env() -> Self {
        let port = env_parse("CC_SWITCH_REMOTE_RELAY_PORT", DEFAULT_PORT);

        let key_path = env::var("CC_SWITCH_REMOTE_RELAY_KEY_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_KEY_PATH));

        let public_ip = env::var("CC_SWITCH_REMOTE_RELAY_PUBLIC_IP")
            .ok()
            .and_then(|raw| match raw.parse::<IpAddr>() {
                Ok(ip) => Some(ip),
                Err(_) => {
                    warn!(value = %raw, "CC_SWITCH_REMOTE_RELAY_PUBLIC_IP 不是合法 IP，已忽略");
                    None
                }
            });

        let defaults = RelayLimits::default();
        let limits = RelayLimits {
            max_reservations: env_parse("CC_SWITCH_REMOTE_RELAY_MAX_RESERVATIONS", defaults.max_reservations),
            max_reservations_per_peer: env_parse(
                "CC_SWITCH_REMOTE_RELAY_MAX_RESERVATIONS_PER_PEER",
                defaults.max_reservations_per_peer,
            ),
            reservation_duration: Duration::from_secs(env_parse(
                "CC_SWITCH_REMOTE_RELAY_RESERVATION_DURATION_SECS",
                defaults.reservation_duration.as_secs(),
            )),
            max_circuits: env_parse("CC_SWITCH_REMOTE_RELAY_MAX_CIRCUITS", defaults.max_circuits),
            max_circuits_per_peer: env_parse(
                "CC_SWITCH_REMOTE_RELAY_MAX_CIRCUITS_PER_PEER",
                defaults.max_circuits_per_peer,
            ),
            max_circuit_duration: Duration::from_secs(env_parse(
                "CC_SWITCH_REMOTE_RELAY_MAX_CIRCUIT_DURATION_SECS",
                defaults.max_circuit_duration.as_secs(),
            )),
            max_circuit_bytes: env_parse(
                "CC_SWITCH_REMOTE_RELAY_MAX_CIRCUIT_BYTES",
                defaults.max_circuit_bytes,
            ),
        };

        Self {
            port,
            key_path,
            public_ip,
            limits,
        }
    }
}

/// 读取并解析环境变量；未设置或解析失败时回退默认值并告警
fn env_parse<T>(key: &str, default: T) -> T
where
    T: std::str::FromStr + std::fmt::Display,
    <T as std::str::FromStr>::Err: std::fmt::Display,
{
    match env::var(key) {
        Ok(raw) => match raw.parse::<T>() {
            Ok(value) => value,
            Err(err) => {
                warn!(%key, value = %raw, %err, "环境变量解析失败，使用默认值 {default}");
                default
            }
        },
        Err(_) => default,
    }
}
