//! libp2p Swarm 装配与事件循环
//!
//! - transport：QUIC（首选）+ TCP（兜底）+ relay circuit（打洞前/兜底）
//! - behaviour：identify（能力通告）+ rendezvous client（发现）+ relay client + DCUtR（打洞升级）+ libp2p-stream（数据面/加入协议）
//! - 事件循环：命令接收、注册续期、周期发现、连接状态跟踪、入站流分发

use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{
    core::{transport::OrTransport, upgrade, Transport as _},
    dcutr, identify, identity,
    multiaddr::Protocol,
    noise, quic, relay, rendezvous,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use super::config;

/// 组网 NetworkBehaviour
#[derive(NetworkBehaviour)]
pub struct ShareBehaviour {
    relay_client: relay::client::Behaviour,
    identify: identify::Behaviour,
    rendezvous: rendezvous::client::Behaviour,
    dcutr: dcutr::Behaviour,
    stream: libp2p_stream::Behaviour,
}

/// 加入请求（JOIN_PROTOCOL 线格式，换行结尾的 JSON）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinRequestWire {
    pub short_code: String,
    pub node_name: String,
}

/// 加入响应（JOIN_PROTOCOL 线格式）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JoinResponseWire {
    pub accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 发送给 swarm 事件循环的命令
pub enum SwarmCmd {
    /// 更新网络配置（加入网络/更换 relay）：拨 relay、预约、注册命名空间
    Configure {
        namespace: Option<String>,
        relay_addrs: Vec<Multiaddr>,
    },
    /// 向指定节点发起加入请求
    SendJoinRequest {
        peer: PeerId,
        request: JoinRequestWire,
        respond: oneshot::Sender<Result<JoinResponseWire, String>>,
    },
    /// 拨号指定地址（测试/手动场景）
    Dial(Multiaddr),
    /// 退出网络（注销命名空间）
    Leave,
    /// 关闭事件循环
    Shutdown,
}

/// 事件循环上送给 ShareManager 的事件
pub enum SwarmEventOut {
    PeerConnected {
        peer_id: PeerId,
        direct: bool,
    },
    PeerDisconnected {
        peer_id: PeerId,
    },
    /// identify 收到对方能力通告（agent_version JSON 中的节点名）
    PeerIdentified {
        peer_id: PeerId,
        name: String,
    },
    /// 数据面入站流（出借侧）
    HttpInbound {
        peer_id: PeerId,
        stream: libp2p::Stream,
    },
    /// 加入申请入站（出借侧；通过 respond 回传审批结果）
    JoinInbound {
        peer_id: PeerId,
        request: JoinRequestWire,
        respond: oneshot::Sender<JoinResponseWire>,
    },
    /// relay 连接/预约状态变化（展示用）
    RelayState {
        connected: bool,
        transport: Option<String>,
    },
}

/// Swarm 句柄（ShareManager 持有）
pub struct SwarmHandle {
    pub cmd: mpsc::Sender<SwarmCmd>,
    pub stream_control: libp2p_stream::Control,
}

/// 传输类型（测试用内存传输）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Real,
    Memory,
}

/// 启动 swarm 事件循环
pub fn start_swarm(
    keypair: identity::Keypair,
    kind: TransportKind,
    node_name: String,
    event_tx: mpsc::Sender<SwarmEventOut>,
    extra_listen: Vec<Multiaddr>,
) -> Result<SwarmHandle, String> {
    let local_peer_id = PeerId::from(keypair.public());

    let (relay_transport, relay_client) = relay::client::new(local_peer_id);

    let mut swarm = match kind {
        TransportKind::Real => {
            // dns 包装使 /dns4/ 域名形式的 QUIC relay 地址可拨（域名优先配置）
            let quic = libp2p::dns::tokio::Transport::system(quic::tokio::Transport::new(
                quic::Config::new(&keypair),
            ))
            .map_err(|e| format!("DNS(quic) transport 初始化失败: {e}"))?
            .map(|(peer, conn), _| (peer, libp2p::core::muxing::StreamMuxerBox::new(conn)));
            let tcp = libp2p::dns::tokio::Transport::system(tcp::tokio::Transport::new(
                tcp::Config::default().nodelay(true),
            ))
            .map_err(|e| format!("DNS transport 初始化失败: {e}"))?
            .upgrade(upgrade::Version::V1Lazy)
            .authenticate(
                noise::Config::new(&keypair).map_err(|e| format!("noise 初始化失败: {e}"))?,
            )
            .multiplex(yamux::Config::default())
            .map(|(peer, muxer), _| (peer, libp2p::core::muxing::StreamMuxerBox::new(muxer)));
            let relayed = relay_transport
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(
                    noise::Config::new(&keypair).map_err(|e| format!("noise 初始化失败: {e}"))?,
                )
                .multiplex(yamux::Config::default())
                .map(|(peer, muxer), _| (peer, libp2p::core::muxing::StreamMuxerBox::new(muxer)));

            // relay client 传输层必须位于 OrTransport 链首位，circuit 地址
            // 才能被正确认领（对齐 rust-libp2p 官方 relay 示例的组网方式）。
            let transport = OrTransport::new(relayed, OrTransport::new(quic, tcp))
                .map(|either, _| match either {
                    futures::future::Either::Left((peer, muxer)) => (peer, muxer),
                    futures::future::Either::Right(futures::future::Either::Left((
                        peer,
                        muxer,
                    ))) => (peer, muxer),
                    futures::future::Either::Right(futures::future::Either::Right((
                        peer,
                        muxer,
                    ))) => (peer, muxer),
                })
                .boxed();

            libp2p::SwarmBuilder::with_existing_identity(keypair.clone())
                .with_tokio()
                .with_other_transport(|_| transport)
                .map_err(|e| format!("构建 swarm 失败: {e}"))?
                .with_behaviour(|kp: &libp2p::identity::Keypair| {
                    build_behaviour(kp, relay_client, &node_name, local_peer_id)
                })
                .map_err(|e| format!("构建 behaviour 失败: {e}"))?
                .with_swarm_config(|cfg: libp2p::swarm::Config| {
                    cfg.with_idle_connection_timeout(std::time::Duration::from_secs(600))
                })
                .build()
        }
        TransportKind::Memory => {
            // 测试用：内存传输（同进程双实例互联）
            // 注意：relay client transport 必须编入组合，否则 relay behaviour 会 panic
            let memory = libp2p::core::transport::MemoryTransport::default()
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(
                    noise::Config::new(&keypair).map_err(|e| format!("noise 初始化失败: {e}"))?,
                )
                .multiplex(yamux::Config::default())
                .map(|(peer, muxer), _| (peer, libp2p::core::muxing::StreamMuxerBox::new(muxer)));
            let relayed = relay_transport
                .upgrade(upgrade::Version::V1Lazy)
                .authenticate(
                    noise::Config::new(&keypair).map_err(|e| format!("noise 初始化失败: {e}"))?,
                )
                .multiplex(yamux::Config::default())
                .map(|(peer, muxer), _| (peer, libp2p::core::muxing::StreamMuxerBox::new(muxer)));
            let transport = OrTransport::new(relayed, memory)
                .map(|either, _| match either {
                    futures::future::Either::Left((peer, muxer)) => (peer, muxer),
                    futures::future::Either::Right((peer, muxer)) => (peer, muxer),
                })
                .boxed();
            libp2p::SwarmBuilder::with_existing_identity(keypair)
                .with_tokio()
                .with_other_transport(|_| transport)
                .map_err(|e| format!("构建 swarm 失败: {e}"))?
                .with_behaviour(|kp: &libp2p::identity::Keypair| {
                    build_behaviour(kp, relay_client, &node_name, local_peer_id)
                })
                .map_err(|e| format!("构建 behaviour 失败: {e}"))?
                .with_swarm_config(|cfg: libp2p::swarm::Config| {
                    cfg.with_idle_connection_timeout(std::time::Duration::from_secs(600))
                })
                .build()
        }
    };

    // 监听本地地址（真实传输：稳定端口，便于 Windows 防火墙规则持久化）
    match kind {
        TransportKind::Real => {
            let port = config::share_p2p_port();
            // 双栈监听：IPv4 通配 + IPv6 通配各自独立注册，任一失败只降级不致命。
            // IPv6 直连是绕开 NAT 中继的主要途径（国内家宽 IPv6 覆盖良好）。
            for template in [
                format!("/ip4/0.0.0.0/udp/{port}/quic-v1"),
                format!("/ip4/0.0.0.0/tcp/{port}"),
                format!("/ip6/::/udp/{port}/quic-v1"),
                format!("/ip6/::/tcp/{port}"),
            ] {
                let addr: Multiaddr = template.parse().expect("listen addr");
                if let Err(error) = swarm.listen_on(addr.clone()) {
                    log::warn!("[Share] P2P 监听失败（{addr}），其余栈继续: {error}");
                }
            }
        }
        TransportKind::Memory => {}
    }
    for addr in extra_listen {
        swarm
            .listen_on(addr)
            .map_err(|e| format!("监听地址失败: {e}"))?;
    }

    // 数据面/加入协议的流控制
    let mut stream_control = swarm.behaviour().stream.new_control();
    let incoming_http = stream_control
        .accept(config::DATA_PLANE_PROTOCOL)
        .map_err(|e| format!("注册数据面协议失败: {e:?}"))?;
    let incoming_join = stream_control
        .accept(config::JOIN_PROTOCOL)
        .map_err(|e| format!("注册加入协议失败: {e:?}"))?;

    let (cmd_tx, cmd_rx) = mpsc::channel::<SwarmCmd>(64);

    tokio::spawn(swarm_loop(
        swarm,
        cmd_rx,
        event_tx,
        incoming_http,
        incoming_join,
        stream_control.clone(),
        kind,
    ));

    Ok(SwarmHandle {
        cmd: cmd_tx,
        stream_control,
    })
}

/// 组装 NetworkBehaviour（两分支共用）
fn build_behaviour(
    keypair: &identity::Keypair,
    relay_client: relay::client::Behaviour,
    node_name: &str,
    local_peer_id: PeerId,
) -> ShareBehaviour {
    ShareBehaviour {
        relay_client,
        identify: identify::Behaviour::new(
            identify::Config::new("/ipfs/id/1.0.0".to_string(), keypair.public())
                .with_agent_version(build_agent_version(node_name)),
        ),
        rendezvous: rendezvous::client::Behaviour::new(keypair.clone()),
        dcutr: dcutr::Behaviour::new(local_peer_id),
        stream: libp2p_stream::Behaviour::new(),
    }
}

/// agent_version 携带能力通告（JSON，长度受 identify 限制，保持精简）
fn build_agent_version(node_name: &str) -> String {
    serde_json::json!({
        "tt": 1,
        "name": node_name,
    })
    .to_string()
}

/// 解析对端 agent_version 中的节点名
fn parse_agent_name(agent_version: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(agent_version).ok()?;
    if v.get("tt")?.as_u64()? >= 1 {
        Some(v.get("name")?.as_str()?.to_string())
    } else {
        None
    }
}

/// 换行分隔 JSON 读写（JOIN 协议线格式，上限 16KB）
async fn write_wire<T: Serialize>(stream: &mut libp2p::Stream, value: &T) -> Result<(), String> {
    let mut line = serde_json::to_vec(value).map_err(|e| format!("序列化失败: {e}"))?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .await
        .map_err(|e| format!("写入失败: {e}"))?;
    stream.flush().await.map_err(|e| format!("flush 失败: {e}"))
}

async fn read_wire<T: for<'de> Deserialize<'de>>(stream: &mut libp2p::Stream) -> Result<T, String> {
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    loop {
        let n = stream
            .read(&mut byte)
            .await
            .map_err(|e| format!("读取失败: {e}"))?;
        if n == 0 {
            return Err("连接已关闭".to_string());
        }
        if byte[0] == b'\n' {
            break;
        }
        buf.push(byte[0]);
        if buf.len() > 16 * 1024 {
            return Err("消息过大".to_string());
        }
    }
    serde_json::from_slice(&buf).map_err(|e| format!("反序列化失败: {e}"))
}

/// 事件循环内部状态
struct LoopState {
    namespace: Option<String>,
    relay_peer: Option<PeerId>,
    relay_addr: Option<Multiaddr>,
    relay_candidates: Vec<Multiaddr>,
    relay_candidate_index: usize,
    relay_retry_at: Instant,
    relay_listener_ids: HashSet<libp2p::core::transport::ListenerId>,
    /// 已建立连接的 peer（可能多条连接：中继 + 直连）
    connections: HashMap<PeerId, HashSet<libp2p::swarm::ConnectionId>>,
    /// 直连中的 peer（存在非中继连接）
    direct: HashSet<PeerId>,
    /// 已尝试拨号的 peer（避免重复轰炸）
    dialed: HashSet<PeerId>,
    /// 每个 peer 的有序地址候选和下一次尝试位置
    peer_candidates: HashMap<PeerId, Vec<Multiaddr>>,
    peer_candidate_index: HashMap<PeerId, usize>,
    /// rendezvous 发现 cookie（增量发现）
    discover_cookie: Option<rendezvous::Cookie>,
    /// 是否已完成 relay 预约
    relay_reserved: bool,
}

#[allow(clippy::too_many_lines)]
async fn swarm_loop(
    mut swarm: Swarm<ShareBehaviour>,
    mut cmd_rx: mpsc::Receiver<SwarmCmd>,
    event_tx: mpsc::Sender<SwarmEventOut>,
    mut incoming_http: libp2p_stream::IncomingStreams,
    mut incoming_join: libp2p_stream::IncomingStreams,
    stream_control: libp2p_stream::Control,
    _kind: TransportKind,
) {
    let local_peer_id = *swarm.local_peer_id();
    let mut state = LoopState {
        namespace: None,
        relay_peer: None,
        relay_addr: None,
        relay_candidates: Vec::new(),
        relay_candidate_index: 0,
        relay_retry_at: Instant::now(),
        relay_listener_ids: HashSet::new(),
        connections: HashMap::new(),
        direct: HashSet::new(),
        dialed: HashSet::new(),
        peer_candidates: HashMap::new(),
        peer_candidate_index: HashMap::new(),
        discover_cookie: None,
        relay_reserved: false,
    };

    let mut renew_tick = tokio::time::interval(std::time::Duration::from_secs(
        config::RENDEZVOUS_RENEW_SECS,
    ));
    let mut discover_tick = tokio::time::interval(std::time::Duration::from_secs(
        config::RENDEZVOUS_DISCOVER_SECS,
    ));
    let mut relay_retry_tick = tokio::time::interval(Duration::from_secs(1));
    // 立即触发一次发现（不等满一个周期）
    discover_tick.reset();

    loop {
        tokio::select! {
            event = swarm.select_next_some() => {
                handle_swarm_event(&mut swarm, &mut state, event, &event_tx, local_peer_id).await;
            }
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SwarmCmd::Configure { namespace, relay_addrs }) => {
                        remove_relay_listeners(&mut swarm, &mut state);
                        // rendezvous cookies are scoped to the namespace used by the
                        // corresponding DISCOVER request.  Reusing a cookie after a
                        // failed join switches to another share id makes the relay
                        // return only changes after the old cursor, so already
                        // registered creators can be missed indefinitely.  Peer
                        // candidates are namespace-scoped for the same reason.
                        state.discover_cookie = None;
                        state.dialed.clear();
                        state.peer_candidates.clear();
                        state.peer_candidate_index.clear();
                        state.namespace = namespace;
                        state.relay_candidates = relay_addrs;
                        state.relay_candidates.sort_by_key(|addr| {
                            if is_quic_addr(addr) { 0 } else { 1 }
                        });
                        state.relay_candidate_index = 0;
                        state.relay_peer = None;
                        state.relay_addr = None;
                        state.relay_reserved = false;
                        state.relay_retry_at = Instant::now();
                        // 只把 dial Ok 视为已入队；实际连接成功/失败由异步事件推进候选。
                        let _ = dial_next_relay(&mut swarm, &mut state);
                        // 尝试注册（外部地址未就绪时会在 NewExternalAddr 后重试）
                        try_register(&mut swarm, &mut state);
                        // 配置完成后立即发现，不等待完整的 30 秒周期。
                        try_discover(&mut swarm, &mut state);
                    }
                    Some(SwarmCmd::SendJoinRequest { peer, request, respond }) => {
                        let mut control = stream_control.clone();
                        tokio::spawn(async move {
                            let result = send_join_request(&mut control, peer, request).await;
                            let _ = respond.send(result);
                        });
                    }
                    Some(SwarmCmd::Dial(addr)) => {
                        if let Err(e) = swarm.dial(addr) {
                            log::debug!("[Share] Dial 失败: {e}");
                        }
                    }
                    Some(SwarmCmd::Leave) => {
                        remove_relay_listeners(&mut swarm, &mut state);
                        if let (Some(ns), Some(relay_peer)) = (state.namespace.take(), state.relay_peer) {
                            if let Ok(namespace) = rendezvous::Namespace::new(ns) {
                                swarm.behaviour_mut().rendezvous.unregister(namespace, relay_peer);
                            }
                        }
                        state.dialed.clear();
                        state.peer_candidates.clear();
                        state.peer_candidate_index.clear();
                        state.relay_peer = None;
                        state.relay_addr = None;
                        state.relay_candidates.clear();
                        state.relay_candidate_index = 0;
                        state.relay_reserved = false;
                        state.discover_cookie = None;
                    }
                    Some(SwarmCmd::Shutdown) | None => {
                        log::info!("[Share] swarm 事件循环退出");
                        return;
                    }
                }
            }
            Some((peer, stream)) = incoming_http.next() => {
                let _ = event_tx
                    .send(SwarmEventOut::HttpInbound { peer_id: peer, stream })
                    .await;
            }
            Some((peer, mut stream)) = incoming_join.next() => {
                let tx = event_tx.clone();
                tokio::spawn(async move {
                    // 读加入申请 → 上送 → 等待审批 → 回写响应
                    let request: JoinRequestWire = match read_wire(&mut stream).await {
                        Ok(r) => r,
                        Err(e) => {
                            log::debug!("[Share] 加入申请读取失败: {e}");
                            return;
                        }
                    };
                    let (respond_tx, respond_rx) = oneshot::channel::<JoinResponseWire>();
                    if tx
                        .send(SwarmEventOut::JoinInbound {
                            peer_id: peer,
                            request,
                            respond: respond_tx,
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                    let response = match tokio::time::timeout(
                        std::time::Duration::from_secs(config::JOIN_REQUEST_TTL_SECS as u64 + 30),
                        respond_rx,
                    )
                    .await
                    {
                        Ok(Ok(resp)) => resp,
                        _ => JoinResponseWire {
                            accepted: false,
                            share_key: None,
                            reason: Some("审批超时".to_string()),
                        },
                    };
                    let _ = write_wire(&mut stream, &response).await;
                });
            }
            _ = renew_tick.tick() => {
                try_register(&mut swarm, &mut state);
            }
            _ = discover_tick.tick() => {
                try_discover(&mut swarm, &mut state);
            }
            _ = relay_retry_tick.tick() => {
                if state.relay_peer.is_none() && Instant::now() >= state.relay_retry_at {
                    state.relay_candidate_index = 0;
                    let _ = dial_next_relay(&mut swarm, &mut state);
                }
            }
        }
    }
}

fn relay_peer_id(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|protocol| match protocol {
        Protocol::P2p(peer_id) => Some(peer_id),
        _ => None,
    })
}

fn remove_relay_listeners(swarm: &mut Swarm<ShareBehaviour>, state: &mut LoopState) {
    for listener_id in state.relay_listener_ids.drain() {
        swarm.remove_listener(listener_id);
    }
}

fn dial_next_relay(swarm: &mut Swarm<ShareBehaviour>, state: &mut LoopState) -> bool {
    while state.relay_candidate_index < state.relay_candidates.len() {
        let index = state.relay_candidate_index;
        state.relay_candidate_index += 1;
        let addr = state.relay_candidates[index].clone();
        let Some(peer) = relay_peer_id(&addr) else {
            log::warn!("[Share] relay 地址缺少 PeerId，跳过: {addr}");
            continue;
        };
        state.relay_peer = Some(peer);
        state.relay_addr = Some(addr.clone());
        match swarm.dial(addr.clone()) {
            Ok(()) => {
                log::info!("[Share] relay 拨号已入队（候选 {}）: {addr}", index + 1);
                return true;
            }
            Err(error) => {
                log::warn!("[Share] relay 拨号同步失败，尝试下一候选: {error}");
                state.relay_peer = None;
                state.relay_addr = None;
            }
        }
    }
    state.relay_peer = None;
    state.relay_addr = None;
    state.relay_retry_at = Instant::now() + Duration::from_secs(5);
    false
}

fn is_relay_addr(addr: &Multiaddr) -> bool {
    addr.iter()
        .any(|protocol| matches!(protocol, Protocol::P2pCircuit))
}

fn is_quic_addr(addr: &Multiaddr) -> bool {
    addr.iter()
        .any(|protocol| matches!(protocol, Protocol::QuicV1))
}

fn ensure_peer_id(addr: Multiaddr, peer: PeerId) -> Option<Multiaddr> {
    // A relay circuit address already contains the relay's PeerId before
    // `/p2p-circuit`, but it still needs the destination PeerId afterwards:
    //
    //   .../p2p/<relay>/p2p-circuit/p2p/<destination>
    //
    // Looking for *any* `/p2p` component mistakes the relay id for the
    // destination.  Dialling the resulting incomplete address later fails with
    // `peer_id: None`, which cannot be associated with the candidate queue and
    // leaves the destination permanently marked as dialled.
    match addr.iter().last() {
        Some(Protocol::P2p(addressed_peer)) if addressed_peer == peer => Some(addr),
        Some(Protocol::P2p(_)) => None,
        Some(Protocol::P2pCircuit) => Some(addr.with(Protocol::P2p(peer))),
        _ if !is_relay_addr(&addr) => Some(addr.with(Protocol::P2p(peer))),
        _ => None,
    }
}

/// 全局单播 IPv6（排除未指定/回环/组播/fe80::/10 链路本地/fc00::/7 唯一本地）。
/// 用分段位运算实现，避免 std 尚未稳定的 is_unicast_global。
fn is_global_unicast_ipv6(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return false;
    }
    let first = ip.segments()[0];
    let link_local = (first & 0xffc0) == 0xfe80;
    let unique_local = (first & 0xfe00) == 0xfc00;
    !link_local && !unique_local
}

fn peer_candidates(
    registration: &rendezvous::Registration,
    relay_addr: Option<&Multiaddr>,
    peer: PeerId,
) -> Vec<Multiaddr> {
    let mut candidates = registration
        .record
        .addresses()
        .iter()
        .cloned()
        .filter_map(|addr| ensure_peer_id(addr, peer))
        .collect::<Vec<_>>();
    if let Some(relay) = relay_addr {
        let circuit = relay
            .clone()
            .with(Protocol::P2pCircuit)
            .with(Protocol::P2p(peer));
        candidates.push(circuit);
    }
    // 拨号优先级：IPv4 直连 QUIC > IPv4 直连 TCP > IPv6 直连 QUIC > IPv6 直连
    // TCP > 中继。家用宽带 IPv6 覆盖参差（无路由/绕路），IPv4 优先能把无效
    // IPv6 尝试的等待时间挪到兜底位置；中继永远最后。
    candidates.sort_by_key(|addr| {
        let ipv4 = addr.iter().any(|p| matches!(p, Protocol::Ip4(_)));
        if is_relay_addr(addr) {
            if is_quic_addr(addr) {
                4
            } else {
                5
            }
        } else if !ipv4 {
            if is_quic_addr(addr) {
                2
            } else {
                3
            }
        } else if is_quic_addr(addr) {
            0
        } else {
            1
        }
    });
    let mut seen = HashSet::new();
    candidates.retain(|addr| seen.insert(addr.clone()));
    candidates
}

fn dial_next_peer(swarm: &mut Swarm<ShareBehaviour>, state: &mut LoopState, peer: PeerId) -> bool {
    let Some(candidates) = state.peer_candidates.get(&peer).cloned() else {
        return false;
    };
    let index = state.peer_candidate_index.entry(peer).or_insert(0);
    while *index < candidates.len() {
        let addr = candidates[*index].clone();
        *index += 1;
        match swarm.dial(addr.clone()) {
            Ok(()) => {
                state.dialed.insert(peer);
                log::info!("[Share] 节点 {peer} 拨号已入队: {addr}");
                return true;
            }
            Err(error) => {
                log::debug!("[Share] 节点 {peer} 候选失败，尝试下一地址: {error}");
            }
        }
    }
    state.dialed.remove(&peer);
    state.peer_candidate_index.remove(&peer);
    false
}

fn try_register(swarm: &mut Swarm<ShareBehaviour>, state: &mut LoopState) {
    let (Some(ns), Some(relay_peer)) = (state.namespace.clone(), state.relay_peer) else {
        return;
    };
    let Ok(namespace) = rendezvous::Namespace::new(ns) else {
        return;
    };
    match swarm
        .behaviour_mut()
        .rendezvous
        .register(namespace, relay_peer, None)
    {
        Ok(()) => log::debug!("[Share] rendezvous 注册请求已发送"),
        Err(rendezvous::client::RegisterError::NoExternalAddresses) => {
            log::debug!("[Share] 外部地址未就绪，注册将在 NewExternalAddr 后重试");
        }
        Err(e) => log::warn!("[Share] rendezvous 注册失败: {e}"),
    }
}

fn try_discover(swarm: &mut Swarm<ShareBehaviour>, state: &mut LoopState) {
    let (Some(ns), Some(relay_peer)) = (state.namespace.clone(), state.relay_peer) else {
        return;
    };
    let Ok(namespace) = rendezvous::Namespace::new(ns) else {
        return;
    };
    swarm.behaviour_mut().rendezvous.discover(
        Some(namespace),
        state.discover_cookie.clone(),
        None,
        relay_peer,
    );
}

async fn send_join_request(
    control: &mut libp2p_stream::Control,
    peer: PeerId,
    request: JoinRequestWire,
) -> Result<JoinResponseWire, String> {
    let mut stream = control
        .open_stream(peer, config::JOIN_PROTOCOL)
        .await
        .map_err(|e| format!("无法连接节点: {e:?}"))?;
    write_wire(&mut stream, &request).await?;
    read_wire(&mut stream).await
}

async fn handle_swarm_event(
    swarm: &mut Swarm<ShareBehaviour>,
    state: &mut LoopState,
    event: SwarmEvent<ShareBehaviourEvent>,
    event_tx: &mpsc::Sender<SwarmEventOut>,
    local_peer_id: PeerId,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            log::info!("[Share] 监听地址: {address}");
            // 以下地址登记为外部地址供 rendezvous 注册发布：
            // 1) relay 电路地址；
            // 2) 全局单播 IPv6 地址（排除 fe80:: 链路本地与 ::1 回环）。IPv6
            //    可直达的对端据此直连，绕开中继。临时地址（Windows 隐私扩展）
            //    会轮换导致注册信息短暂过期，代价仅为对端拨号该候选失败后
            //    顺延到下一候选，由周期注册与 identify 观察自行修正。
            let is_circuit = address.iter().any(|p| matches!(p, Protocol::P2pCircuit));
            let is_global_ipv6 = address
                .iter()
                .find_map(|p| match p {
                    Protocol::Ip6(ip) => Some(ip),
                    _ => None,
                })
                .is_some_and(is_global_unicast_ipv6);
            if is_circuit || is_global_ipv6 {
                swarm.add_external_address(address);
                try_register(swarm, state);
            }
        }
        SwarmEvent::ConnectionEstablished {
            peer_id,
            endpoint,
            connection_id,
            ..
        } => {
            let direct = !endpoint.is_relayed();
            state
                .connections
                .entry(peer_id)
                .or_default()
                .insert(connection_id);
            if direct {
                state.direct.insert(peer_id);
            }
            let is_direct = state.direct.contains(&peer_id);
            if Some(peer_id) == state.relay_peer {
                if !state.relay_reserved {
                    let transport = state
                        .relay_addr
                        .as_ref()
                        .map(|addr| if is_quic_addr(addr) { "QUIC" } else { "TCP" })
                        .unwrap_or("unknown");
                    log::info!("[Share] relay 控制连接已建立（{transport}）: {peer_id}");
                    if let Some(relay_addr) = state.relay_addr.clone() {
                        let circuit = relay_addr.with(Protocol::P2pCircuit);
                        match swarm.listen_on(circuit) {
                            Ok(listener_id) => {
                                state.relay_listener_ids.insert(listener_id);
                                state.relay_reserved = true;
                                // Configure can run before the relay control
                                // connection exists.  Retry discovery immediately
                                // once the connection is usable instead of making a
                                // new join wait for the periodic 30-second tick.
                                try_register(swarm, state);
                                try_discover(swarm, state);
                                let _ = event_tx
                                    .send(SwarmEventOut::RelayState {
                                        connected: true,
                                        transport: Some(transport.to_ascii_lowercase()),
                                    })
                                    .await;
                            }
                            Err(e) => {
                                log::warn!("[Share] relay 预约失败，尝试下一候选: {e}");
                                state.relay_peer = None;
                                state.relay_addr = None;
                                let _ = dial_next_relay(swarm, state);
                            }
                        }
                    }
                }
            } else {
                log::info!(
                    "[Share] 节点已连接: {peer_id}（{}）",
                    if is_direct { "直连" } else { "中继" }
                );
                let _ = event_tx
                    .send(SwarmEventOut::PeerConnected {
                        peer_id,
                        direct: is_direct,
                    })
                    .await;
            }
        }
        SwarmEvent::ConnectionClosed {
            peer_id,
            connection_id,
            ..
        } => {
            if let Some(conns) = state.connections.get_mut(&peer_id) {
                conns.remove(&connection_id);
                if conns.is_empty() {
                    state.connections.remove(&peer_id);
                    state.direct.remove(&peer_id);
                    state.dialed.remove(&peer_id);
                    if Some(peer_id) == state.relay_peer {
                        state.relay_reserved = false;
                        state.relay_peer = None;
                        state.relay_addr = None;
                        state.relay_retry_at = Instant::now() + Duration::from_secs(1);
                        let _ = event_tx
                            .send(SwarmEventOut::RelayState {
                                connected: false,
                                transport: None,
                            })
                            .await;
                    } else {
                        state.peer_candidate_index.remove(&peer_id);
                        let _ = event_tx
                            .send(SwarmEventOut::PeerDisconnected { peer_id })
                            .await;
                    }
                }
            }
        }
        SwarmEvent::Behaviour(ShareBehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info,
            ..
        })) => {
            if Some(peer_id) == state.relay_peer {
                // relay 的 identify 只用于地址/连接诊断，不是共享网络成员。
                swarm.add_external_address(info.observed_addr);
                return;
            }
            // 对方观察到的我们的地址 → 作为外部地址（打洞/注册用）
            swarm.add_external_address(info.observed_addr);
            if let Some(name) = parse_agent_name(&info.agent_version) {
                let _ = event_tx
                    .send(SwarmEventOut::PeerIdentified { peer_id, name })
                    .await;
            }
        }
        SwarmEvent::Behaviour(ShareBehaviourEvent::Rendezvous(
            rendezvous::client::Event::Discovered {
                registrations,
                cookie,
                ..
            },
        )) => {
            state.discover_cookie.replace(cookie);
            for registration in registrations {
                let peer = registration.record.peer_id();
                if peer == local_peer_id || state.connections.contains_key(&peer) {
                    continue;
                }
                if !state.dialed.insert(peer) {
                    continue;
                }
                let candidates = peer_candidates(&registration, state.relay_addr.as_ref(), peer);
                if candidates.is_empty() {
                    state.dialed.remove(&peer);
                    continue;
                }
                state.peer_candidates.insert(peer, candidates);
                state.peer_candidate_index.insert(peer, 0);
                let _ = dial_next_peer(swarm, state, peer);
            }
        }
        SwarmEvent::Behaviour(ShareBehaviourEvent::Rendezvous(
            rendezvous::client::Event::RegisterFailed { error, .. },
        )) => {
            log::warn!("[Share] rendezvous 注册被拒绝: {error:?}");
        }
        SwarmEvent::Behaviour(ShareBehaviourEvent::Rendezvous(
            rendezvous::client::Event::DiscoverFailed { error, .. },
        )) => {
            log::warn!("[Share] rendezvous 发现失败: {error:?}");
        }
        SwarmEvent::Behaviour(ShareBehaviourEvent::Dcutr(dcutr::Event {
            remote_peer_id,
            result,
            ..
        })) => match result {
            Ok(_) => log::info!("[Share] 打洞成功（直连升级）: {remote_peer_id}"),
            Err(e) => log::info!("[Share] 打洞失败（保持中继）: {remote_peer_id}: {e}"),
        },
        SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
            log::debug!("[Share] 外拨失败 {peer_id:?}: {error}");
            let Some(peer) = peer_id else {
                return;
            };
            if Some(peer) == state.relay_peer {
                state.relay_peer = None;
                state.relay_addr = None;
                state.relay_reserved = false;
                let _ = dial_next_relay(swarm, state);
            } else if state.peer_candidates.contains_key(&peer) {
                state.dialed.remove(&peer);
                let _ = dial_next_peer(swarm, state, peer);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_circuit_address_appends_destination_peer_id() {
        let relay = PeerId::from(identity::Keypair::generate_ed25519().public());
        let destination = PeerId::from(identity::Keypair::generate_ed25519().public());
        let circuit: Multiaddr =
            format!("/ip4/192.0.2.10/udp/15720/quic-v1/p2p/{relay}/p2p-circuit")
                .parse()
                .unwrap();

        let completed = ensure_peer_id(circuit, destination).expect("complete circuit address");

        assert_eq!(
            completed.to_string(),
            format!("/ip4/192.0.2.10/udp/15720/quic-v1/p2p/{relay}/p2p-circuit/p2p/{destination}")
        );
    }

    #[test]
    fn relay_circuit_address_with_destination_is_preserved() {
        let relay = PeerId::from(identity::Keypair::generate_ed25519().public());
        let destination = PeerId::from(identity::Keypair::generate_ed25519().public());
        let addressed: Multiaddr =
            format!("/ip4/192.0.2.10/udp/15720/quic-v1/p2p/{relay}/p2p-circuit/p2p/{destination}")
                .parse()
                .unwrap();

        assert_eq!(
            ensure_peer_id(addressed.clone(), destination),
            Some(addressed)
        );
    }

    #[test]
    fn direct_address_appends_destination_peer_id() {
        let destination = PeerId::from(identity::Keypair::generate_ed25519().public());
        let direct: Multiaddr = "/ip4/192.0.2.11/tcp/49152".parse().unwrap();

        let completed = ensure_peer_id(direct, destination).expect("complete direct address");

        assert_eq!(
            completed.to_string(),
            format!("/ip4/192.0.2.11/tcp/49152/p2p/{destination}")
        );
    }

    #[test]
    fn address_with_wrong_final_peer_is_rejected() {
        let destination = PeerId::from(identity::Keypair::generate_ed25519().public());
        let other = PeerId::from(identity::Keypair::generate_ed25519().public());
        let addressed_to_other: Multiaddr = format!("/ip4/192.0.2.12/tcp/49153/p2p/{other}")
            .parse()
            .unwrap();

        assert!(ensure_peer_id(addressed_to_other, destination).is_none());
    }

    /// 双内存 swarm 全链路：
    /// 连接建立 → 数据面 stream echo → 加入申请/审批 wire 往返
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn memory_swarm_full_roundtrip() {
        let kp_a = identity::Keypair::generate_ed25519();
        let kp_b = identity::Keypair::generate_ed25519();
        let peer_b = PeerId::from(kp_b.public());

        let (tx_a, mut rx_a) = mpsc::channel::<SwarmEventOut>(64);
        let (tx_b, mut rx_b) = mpsc::channel::<SwarmEventOut>(64);
        let handle_a = start_swarm(
            kp_a,
            TransportKind::Memory,
            "node-a".into(),
            tx_a,
            vec!["/memory/110".parse().unwrap()],
        )
        .unwrap();
        let _handle_b = start_swarm(
            kp_b,
            TransportKind::Memory,
            "node-b".into(),
            tx_b,
            vec!["/memory/220".parse().unwrap()],
        )
        .unwrap();

        // A 侧事件收集：等到 PeerConnected
        let (conn_tx_a, conn_rx_a) = oneshot::channel::<bool>();
        tokio::spawn(async move {
            let mut conn_tx_a = Some(conn_tx_a);
            while let Some(ev) = rx_a.recv().await {
                if matches!(ev, SwarmEventOut::PeerConnected { .. }) {
                    if let Some(tx) = conn_tx_a.take() {
                        let _ = tx.send(true);
                    }
                }
            }
        });

        // B 侧事件处理：连接通知 + 数据面 echo + 加入申请自动批准
        let (conn_tx_b, conn_rx_b) = oneshot::channel::<bool>();
        tokio::spawn(async move {
            let mut conn_tx_b = Some(conn_tx_b);
            while let Some(ev) = rx_b.recv().await {
                match ev {
                    SwarmEventOut::PeerConnected { .. } => {
                        if let Some(tx) = conn_tx_b.take() {
                            let _ = tx.send(true);
                        }
                    }
                    SwarmEventOut::HttpInbound { mut stream, .. } => {
                        tokio::spawn(async move {
                            let mut buf = [0u8; 4];
                            if stream.read_exact(&mut buf).await.is_ok() {
                                let _ = stream.write_all(&buf).await;
                                let _ = stream.flush().await;
                            }
                        });
                    }
                    SwarmEventOut::JoinInbound { respond, .. } => {
                        let _ = respond.send(JoinResponseWire {
                            accepted: true,
                            share_key: Some("dGVzdC1rZXk".to_string()),
                            reason: None,
                        });
                    }
                    _ => {}
                }
            }
        });

        // A 拨 B
        handle_a
            .cmd
            .send(SwarmCmd::Dial("/memory/220".parse().unwrap()))
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), conn_rx_a)
            .await
            .expect("A 侧连接超时")
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(15), conn_rx_b)
            .await
            .expect("B 侧连接超时")
            .unwrap();

        // 数据面 echo
        let mut stream = handle_a
            .stream_control
            .clone()
            .open_stream(peer_b, config::DATA_PLANE_PROTOCOL)
            .await
            .expect("open data stream");
        stream.write_all(b"ping").await.unwrap();
        stream.flush().await.unwrap();
        let mut buf = [0u8; 4];
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            stream.read_exact(&mut buf),
        )
        .await
        .expect("echo 超时")
        .unwrap();
        assert_eq!(&buf, b"ping");
        drop(stream);

        // 加入申请 → 审批（完整 wire 往返 + key 下发）
        let (resp_tx, resp_rx) = oneshot::channel();
        handle_a
            .cmd
            .send(SwarmCmd::SendJoinRequest {
                peer: peer_b,
                request: JoinRequestWire {
                    short_code: "482913".into(),
                    node_name: "node-a".into(),
                },
                respond: resp_tx,
            })
            .await
            .unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(15), resp_rx)
            .await
            .expect("join 应答超时")
            .expect("join 通道关闭")
            .expect("join 请求失败");
        assert!(response.accepted);
        assert_eq!(response.share_key.as_deref(), Some("dGVzdC1rZXk"));
    }
}
