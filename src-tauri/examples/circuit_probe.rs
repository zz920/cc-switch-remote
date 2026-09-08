//! 电路中继诊断：连 relay → 发现命名空间 → 逐个拨号候选（重点 circuit），
//! 打印全部 swarm 事件。用法：cargo run --example circuit_probe -- [namespace]
use futures::StreamExt;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{identify, identity, quic, relay, rendezvous, yamux};

const RELAY_ADDR: &str = "/ip4/47.93.197.182/udp/15720/quic-v1/p2p/12D3KooWJviFfoWKwmhamp8Tsrx5GmDgGnoo3BTiAgqaLzRT7Qcz";

#[derive(NetworkBehaviour)]
struct Probe {
    identify: identify::Behaviour,
    rendezvous: rendezvous::client::Behaviour,
    relay: relay::client::Behaviour,
}

#[tokio::main]
async fn main() {
    let namespace = std::env::args().nth(1).unwrap_or_else(|| "cc-switch-remote:600102bfffa3c124".into());
    let keypair = identity::Keypair::generate_ed25519();
    let peer_id = keypair.public().to_peer_id();
    println!("[probe] 本机 peer = {peer_id}");

    let relay_key = identity::Keypair::generate_ed25519();
    let (relay_transport, relay_client) = relay::client::new(peer_id);

    let quic = quic::tokio::Transport::new(quic::Config::new(&keypair));
    let tcp = libp2p::tcp::tokio::Transport::default::default();
    let relayed = relay_transport
        .upgrade(libp2p::core::upgrade::Version::V1Lazy)
        .authenticate(libp2p::noise::Config::new(&keypair).unwrap())
        .multiplex(yamux::Config::default())
        .map(|(p, m), _| (p, libp2p::core::muxing::StreamMuxerBox::new(m)));
    let tcp = tcp
        .upgrade(libp2p::core::upgrade::Version::V1Lazy)
        .authenticate(libp2p::noise::Config::new(&keypair).unwrap())
        .multiplex(yamux::Config::default())
        .map(|(p, m), _| (p, libp2p::core::muxing::StreamMuxerBox::new(m)));

    let transport = libp2p::core::transport::OrTransport::new(quic, libp2p::core::transport::OrTransport::new(tcp, relayed))
        .map(|either, _| match either {
            futures::future::Either::Left((p, m)) => (p, m),
            futures::future::Either::Right(futures::future::Either::Left((p, m))) => (p, m),
            futures::future::Either::Right(futures::future::Either::Right((p, m))) => (p, m),
        })
        .boxed();

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_other_transport(|_| transport)
        .unwrap()
        .with_behaviour(|kp: &identity::Keypair| {
            let _ = &relay_key;
            Probe {
                identify: identify::Behaviour::new(identify::Config::new("/ipfs/id/1.0.0".into(), kp.public())),
                rendezvous: rendezvous::client::Behaviour::new(identity::Keypair::generate_ed25519()),
                relay: relay_client,
            }
        })
        .unwrap()
        .build();

    swarm.dial(RELAY_ADDR.parse().unwrap()).unwrap();
    println!("[probe] 已拨 relay，监听事件 90s…");

    let mut dialed = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    while let Some(event) = swarm.next().await {
        match event {
            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } => {
                let relayed = endpoint.is_relayed();
                println!("[probe] ✅ 连接建立 {peer_id} relayed={relayed}");
                if !relayed {
                    // 假定为 relay 控制连接：发起发现
                    if let Ok(ns) = rendezvous::Namespace::new(namespace.clone()) {
                        swarm.behaviour_mut().rendezvous.discover(Some(ns), None, None, peer_id);
                        println!("[probe] 已发起 discover namespace={namespace}");
                    }
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                println!("[probe] ❌ 外拨失败 {peer_id:?}: {error}");
            }
            SwarmEvent::Behaviour(ProbeEvent::Rendezvous(rendezvous::client::Event::Discovered { registrations, .. })) => {
                for reg in registrations {
                    let peer = reg.record.peer_id();
                    if peer == peer_id || dialed.contains(&peer) { continue; }
                    println!("[probe] 🔍 发现 {peer}，地址:");
                    let mut cands: Vec<_> = reg.record.addresses().to_vec();
                    if let Some(relay_peer) = RELAY_ADDR.parse::<libp2p::Multiaddr>().unwrap().iter().find_map(|p| match p { Protocol::P2p(id) => Some(id), _ => None }) {
                        let base: libp2p::Multiaddr = "/ip4/47.93.197.182/udp/15720/quic-v1".parse().unwrap();
                        cands.push(base.with(Protocol::P2p(relay_peer)).with(Protocol::P2pCircuit).with(Protocol::P2p(peer)));
                    }
                    for addr in &cands { println!("    {addr}"); }
                    for addr in cands {
                        dialed.insert(peer);
                        match swarm.dial(addr.clone()) {
                            Ok(()) => println!("[probe] 拨号入队: {addr}"),
                            Err(e) => println!("[probe] 拨号拒绝 {addr}: {e}"),
                        }
                    }
                }
            }
            SwarmEvent::Behaviour(ProbeEvent::Relay(relay::client::Event::ReservationReqAccepted { .. })) => {
                println!("[probe] 🎫 relay 预约已接受");
            }
            SwarmEvent::Behaviour(ProbeEvent::Rendezvous(rendezvous::client::Event::Registered { namespace, .. })) => {
                println!("[probe] 注册成功 {namespace}");
            }
            other => {
                if std::time::Instant::now() > deadline { break; }
                let _ = other;
            }
        }
        if std::time::Instant::now() > deadline { break; }
    }
    println!("[probe] 结束");
}
