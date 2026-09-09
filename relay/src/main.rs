//! cc-switch-remote 组网信令/中继服务器
//!
//! 职责（无业务逻辑、无状态）：
//! 1. rendezvous 服务端 —— cc-switch-remote 客户端按 share id 命名空间注册/发现彼此（信令）；
//! 2. circuit relay v2 服务端 —— DCUtR 打洞失败时在两客户端之间中继流量（兜底）；
//! 3. identify —— 与客户端交换观察到的公网地址等信息（打洞流程依赖）。
//!
//! 传输层同时监听 QUIC（UDP，首选）与 TCP + Noise + yamux（UDP 被封锁时的兜底）。

use std::{error::Error, net::IpAddr};

use futures::StreamExt;
use libp2p::{
    Multiaddr, identify, noise, relay, rendezvous,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

mod config;
mod keypair;

/// identify 协议标识：与 cc-switch-remote 客户端保持一致（沿用 libp2p 标准值，
/// 保证与官方 dcutr/relay 示例代码的互操作性）
const IDENTIFY_PROTOCOL: &str = "/ipfs/id/1.0.0";
/// identify 上报的 agent 版本，标识本实现
const AGENT_VERSION: &str = concat!("cc-switch-remote-relay/", env!("CARGO_PKG_VERSION"));

/// 组合网络行为：中继 + 信令 + 身份交换
#[derive(NetworkBehaviour)]
struct ServerBehaviour {
    relay: relay::Behaviour,
    rendezvous: rendezvous::server::Behaviour,
    identify: identify::Behaviour,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 日志：默认 info，可用 RUST_LOG 覆盖（如 RUST_LOG=debug）
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = config::Config::from_env();
    let port = cfg.port;

    // 加载或生成节点身份（重启后 PeerId 不变）
    let local_key = keypair::load_or_generate(&cfg.key_path)?;
    let local_peer_id = local_key.public().to_peer_id();

    let relay_config = relay::Config::from(&cfg.limits);

    // 装配 Swarm：QUIC + TCP(Noise+yamux) 双 transport
    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(local_key)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| ServerBehaviour {
            relay: relay::Behaviour::new(key.public().to_peer_id(), relay_config),
            rendezvous: rendezvous::server::Behaviour::new(
                rendezvous::server::Config::default(),
            ),
            identify: identify::Behaviour::new(
                identify::Config::new(IDENTIFY_PROTOCOL.to_string(), key.public())
                    .with_agent_version(AGENT_VERSION.to_string()),
            ),
        })?
        .build();

    // 监听双 transport（同一端口号）
    let quic_listen: Multiaddr = format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse()?;
    let tcp_listen: Multiaddr = format!("/ip4/0.0.0.0/tcp/{port}").parse()?;
    swarm.listen_on(quic_listen.clone())?;
    swarm.listen_on(tcp_listen.clone())?;

    // 若通过环境变量显式指定了公网 IP，注册为 external address：
    // relay 在接受预约时会把这些地址回告客户端，供其他节点经中继拨号
    if let Some(ip) = cfg.public_ip {
        for addr in public_addrs(ip, port) {
            info!(%addr, "注册公网 external address");
            swarm.add_external_address(addr);
        }
    }

    print_banner(&local_peer_id.to_string(), port, cfg.public_ip, &cfg.limits);

    // 主事件循环：驱动 Swarm，直到收到 Ctrl-C
    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_swarm_event(&mut swarm, event);
            }
            result = tokio::signal::ctrl_c() => {
                match result {
                    Ok(()) => info!("收到 Ctrl-C，正在优雅退出..."),
                    Err(err) => warn!(%err, "监听 Ctrl-C 失败，直接退出"),
                }
                break;
            }
        }
    }

    info!("已退出");
    Ok(())
}

/// 根据公网 IP 生成两种 transport 的完整 external address
fn public_addrs(ip: IpAddr, port: u16) -> [Multiaddr; 2] {
    match ip {
        IpAddr::V4(v4) => [
            format!("/ip4/{v4}/udp/{port}/quic-v1").parse().expect("合法的 multiaddr"),
            format!("/ip4/{v4}/tcp/{port}").parse().expect("合法的 multiaddr"),
        ],
        IpAddr::V6(v6) => [
            format!("/ip6/{v6}/udp/{port}/quic-v1").parse().expect("合法的 multiaddr"),
            format!("/ip6/{v6}/tcp/{port}").parse().expect("合法的 multiaddr"),
        ],
    }
}

/// 处理 Swarm 事件（纯日志 + 维护 external address，无业务状态）
fn handle_swarm_event(
    swarm: &mut libp2p::Swarm<ServerBehaviour>,
    event: SwarmEvent<ServerBehaviourEvent>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            info!(%address, "开始监听");
        }
        SwarmEvent::Behaviour(ServerBehaviourEvent::Identify(identify_event)) => {
            handle_identify_event(swarm, identify_event);
        }
        SwarmEvent::Behaviour(ServerBehaviourEvent::Relay(relay_event)) => {
            handle_relay_event(relay_event);
        }
        SwarmEvent::Behaviour(ServerBehaviourEvent::Rendezvous(rz_event)) => {
            handle_rendezvous_event(rz_event);
        }
        SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
            debug!(%peer_id, ?endpoint, "连接建立");
        }
        SwarmEvent::ConnectionClosed { peer_id, cause, .. } => {
            debug!(%peer_id, ?cause, "连接关闭");
        }
        SwarmEvent::IncomingConnectionError { error, .. } => {
            debug!(%error, "入站连接失败");
        }
        SwarmEvent::OutgoingConnectionError { error, .. } => {
            debug!(%error, "出站连接失败");
        }
        _ => {}
    }
}

/// identify 事件：把客户端观察到的本机地址注册为 external address，
/// 使 relay 能把自己的公网可达地址回告给后续预约的客户端
fn handle_identify_event(
    swarm: &mut libp2p::Swarm<ServerBehaviour>,
    event: identify::Event,
) {
    if let identify::Event::Received { peer_id, info, .. } = event {
        debug!(%peer_id, observed = %info.observed_addr, "收到 identify 信息");
        swarm.add_external_address(info.observed_addr);
    }
}

/// relay 事件日志（已废弃的事件变体统一走 `_` 忽略，避免 deprecated 告警）
fn handle_relay_event(event: relay::Event) {
    match event {
        relay::Event::ReservationReqAccepted { src_peer_id, renewed } => {
            info!(peer = %src_peer_id, renewed, "中继预约已接受");
        }
        relay::Event::ReservationReqDenied { src_peer_id, status } => {
            warn!(peer = %src_peer_id, ?status, "中继预约被拒绝（可能触发资源限制）");
        }
        relay::Event::ReservationClosed { src_peer_id } => {
            debug!(peer = %src_peer_id, "中继预约已关闭");
        }
        relay::Event::ReservationTimedOut { src_peer_id } => {
            debug!(peer = %src_peer_id, "中继预约已过期");
        }
        relay::Event::CircuitReqAccepted { src_peer_id, dst_peer_id } => {
            info!(src = %src_peer_id, dst = %dst_peer_id, "中继电路已建立");
        }
        relay::Event::CircuitReqDenied { src_peer_id, dst_peer_id, status } => {
            warn!(src = %src_peer_id, dst = %dst_peer_id, ?status, "中继电路请求被拒绝");
        }
        relay::Event::CircuitClosed { src_peer_id, dst_peer_id, error } => {
            debug!(src = %src_peer_id, dst = %dst_peer_id, ?error, "中继电路已关闭");
        }
        // ReservationReqAcceptFailed / ReservationReqDenyFailed /
        // CircuitReqDenyFailed / CircuitReqOutboundConnectFailed /
        // CircuitReqAcceptFailed 在上游已标记 deprecated，此处忽略
        _ => {}
    }
}

/// rendezvous 服务端事件日志
fn handle_rendezvous_event(event: rendezvous::server::Event) {
    use rendezvous::server::Event;
    match event {
        Event::PeerRegistered { peer, registration } => {
            info!(%peer, namespace = %registration.namespace, ttl = registration.ttl, "节点已注册");
        }
        Event::PeerNotRegistered { peer, namespace, error } => {
            warn!(%peer, %namespace, ?error, "节点注册被拒绝");
        }
        Event::PeerUnregistered { peer, namespace } => {
            info!(%peer, %namespace, "节点已注销");
        }
        Event::DiscoverServed { enquirer, registrations } => {
            debug!(%enquirer, count = registrations.len(), "发现请求已应答");
        }
        Event::DiscoverNotServed { enquirer, error } => {
            debug!(%enquirer, ?error, "发现请求未应答");
        }
        Event::RegistrationExpired(registration) => {
            debug!(namespace = %registration.namespace, "注册已过期");
        }
    }
}

/// 启动横幅：打印 PeerId 与客户端 bootstrap 地址（公网 IP 未知时用占位符）
fn print_banner(peer_id: &str, port: u16, public_ip: Option<IpAddr>, limits: &config::RelayLimits) {
    let ip = public_ip
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| "<PUBLIC_IP>".to_string());

    println!();
    println!("================ cc-switch-remote Relay ================");
    println!("PeerId: {peer_id}");
    println!("监听:   /ip4/0.0.0.0/udp/{port}/quic-v1");
    println!("        /ip4/0.0.0.0/tcp/{port}");
    println!();
    println!("客户端 bootstrap 地址（ CC_SWITCH_REMOTE_RELAY_ADDR ）:");
    println!("  /ip4/{ip}/udp/{port}/quic-v1/p2p/{peer_id}");
    println!("  /ip4/{ip}/tcp/{port}/p2p/{peer_id}");
    if public_ip.is_none() {
        println!("（请将 <PUBLIC_IP> 替换为本机公网 IP；或设置 CC_SWITCH_REMOTE_RELAY_PUBLIC_IP 后重启以自动补全）");
    }
    println!();
    println!("中继资源限制:");
    println!("  预约: 全服 {} / 每节点 {} / 有效期 {:?}", limits.max_reservations, limits.max_reservations_per_peer, limits.reservation_duration);
    println!("  电路: 全服 {} / 每节点 {} / 单路最长 {:?} / 单路最大 {} MiB",
        limits.max_circuits,
        limits.max_circuits_per_peer,
        limits.max_circuit_duration,
        limits.max_circuit_bytes / (1 << 20),
    );
    println!("================================================");
    println!();
}
