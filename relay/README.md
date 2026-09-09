# cc-switch-remote-relay

cc-switch-remote 组网的**信令 / 中继服务器**。基于 [rust-libp2p](https://github.com/libp2p/rust-libp2p) 实现，为 NAT 后的 cc-switch-remote 桌面客户端提供：

1. **节点发现（信令）** —— `rendezvous` 服务端：客户端按 `cc-switch-remote:{sha256(share_id)[:16]}` 命名空间注册自己、发现同伴；
2. **流量中继（兜底）** —— circuit relay v2 服务端：QUIC + DCUtR 打洞失败时，在两客户端之间中转加密流量；
3. **地址交换** —— `identify`：交换观察到的公网地址，是 DCUtR 打洞流程的必备环节。

服务器本身**无业务逻辑、无磁盘状态**（除身份密钥文件），不感知 share id、不解析任何应用层数据——中继的逐跳加密仅由 libp2p transport（QUIC-TLS / Noise）保证，端到端内容对 relay 不可见。

## 技术栈

| 组件 | 说明 |
|---|---|
| libp2p | `0.56.0`（crates.io 最新稳定版） |
| transport | QUIC（UDP，首选）+ TCP（Noise 加密 + yamux 多路复用，UDP 被封锁时兜底） |
| behaviour | `rendezvous::server`、`relay`（circuit relay v2 服务端）、`identify` |
| 节点身份 | Ed25519 密钥对，持久化于 `relay.key`（0600），重启后 PeerId 不变 |
| 运行时 | tokio（rt-multi-thread） |
| 日志 | tracing + tracing-subscriber（`RUST_LOG` 控制） |

## 构建

```bash
cargo build --release
# 产物：target/release/cc-switch-remote-relay
```

要求 Rust 工具链 ≥ 1.83。

## 运行

直接运行（前台调试）：

```bash
RUST_LOG=info ./target/release/cc-switch-remote-relay
```

启动横幅会打印本机 PeerId 与客户端 bootstrap 地址：

```
================ cc-switch-remote Relay ================
PeerId: 12D3KooWFx7wJkoHh8UGgVZUCHi5bN445g5YYgW2CLKbEqP4pxGb
监听:   /ip4/0.0.0.0/udp/15720/quic-v1
        /ip4/0.0.0.0/tcp/15720

客户端 bootstrap 地址（ CC_SWITCH_REMOTE_RELAY_ADDR ）:
  /ip4/<PUBLIC_IP>/udp/15720/quic-v1/p2p/12D3KooWFx7wJkoHh8UGgVZUCHi5bN445g5YYgW2CLKbEqP4pxGb
  /ip4/<PUBLIC_IP>/tcp/15720/p2p/12D3KooWFx7wJkoHh8UGgVZUCHi5bN445g5YYgW2CLKbEqP4pxGb
================================================
```

`Ctrl-C` 优雅退出。

### 环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `CC_SWITCH_REMOTE_RELAY_PORT` | `15720` | 监听端口（QUIC/UDP 与 TCP 共用） |
| `CC_SWITCH_REMOTE_RELAY_KEY_PATH` | `./relay.key` | 身份密钥文件路径（0600，首次启动自动生成） |
| `CC_SWITCH_REMOTE_RELAY_PUBLIC_IP` | 未设置 | 服务器公网 IP。设置后注册为 external address，relay 在接受预约时将其回告客户端；横幅中的 bootstrap 地址也会自动补全 |
| `RUST_LOG` | `info` | 日志级别（`debug` 可见每个连接/注册细节） |

## 部署（systemd）

一键部署（编译 → 安装到 `/opt/cc-switch-remote-relay` → 创建 `cc-switch-remote` 系统用户 → 注册并启动服务）：

```bash
sudo ./deploy/deploy.sh
```

脚本结尾会打印防火墙放行命令（ufw / firewalld 二选一），例如：

```bash
# ufw
sudo ufw allow 15720/tcp comment 'cc-switch-remote-relay'
sudo ufw allow 15720/udp comment 'cc-switch-remote-relay quic'

# firewalld
sudo firewall-cmd --permanent --add-port=15720/tcp --add-port=15720/udp
sudo firewall-cmd --reload
```

> 云服务器还需在**安全组**中放行 `15720` 的 TCP 与 UDP 入站。

手动部署要点（等价于脚本所做）：

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin cc-switch-remote
sudo install -d -o cc-switch-remote -g cc-switch-remote /opt/cc-switch-remote-relay
sudo install -o cc-switch-remote -g cc-switch-remote -m 0755 target/release/cc-switch-remote-relay /opt/cc-switch-remote-relay/
sudo install -m 0644 deploy/cc-switch-remote-relay.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now cc-switch-remote-relay
```

- 服务以**非 root** 的 `cc-switch-remote` 用户运行，`Restart=always`，`LimitNOFILE=65536`；
- 密钥文件 `/opt/cc-switch-remote-relay/relay.key` 首次启动自动生成（0600，属主 `cc-switch-remote`）。**备份该文件即可在迁移机器后保持 PeerId / bootstrap 地址不变**；
- 服务器有固定公网 IP 时，建议在 unit 中设置 `Environment=CC_SWITCH_REMOTE_RELAY_PUBLIC_IP=<IP>`；
- 常用命令：`journalctl -u cc-switch-remote-relay -f` 看日志，`systemctl restart cc-switch-remote-relay` 重启。

## 客户端如何指向自建 relay

启动日志中的完整 multiaddr 即 bootstrap 地址，两种 transport 任填其一（QUIC 优先）：

```
/ip4/<PUBLIC_IP>/udp/15720/quic-v1/p2p/<PEER_ID>
/ip4/<PUBLIC_IP>/tcp/15720/p2p/<PEER_ID>
```

cc-switch-remote 客户端配置方式（二选一）：

1. 环境变量：

   ```bash
   export CC_SWITCH_REMOTE_RELAY_ADDR=/ip4/203.0.113.10/udp/15720/quic-v1/p2p/12D3KooW...
   ```

2. 「设置 → 共享网络 → relay 地址」设置项：粘贴同一 multiaddr，校验连通性后生效；可随时一键恢复官方默认 relay。

客户端侧的约定（与本服务器对齐，主程序开发时遵循）：

- identify 协议标识使用 libp2p 标准值 `/ipfs/id/1.0.0`（agent_version 自定义为 `cc-switch-remote/...`）；
- rendezvous 命名空间：`cc-switch-remote:{sha256(share_id)[:16]}`，注册 TTL 需在服务器允许范围内（默认 2h–72h，客户端建议取 2h 并周期续期）；
- 打洞失败时通过 relay 预约 + `/p2p-circuit` 地址建立中继电路。

## 安全与资源限制

relay 只应是打洞失败时的**兜底路径**。为防止服务器被当成免费无限带宽滥用，circuit relay v2 启用了以下硬上限（`src/config.rs`，均可环境变量覆盖）：

| 限制项 | 默认值 | 环境变量 | 含义 |
|---|---|---|---|
| `max_reservations` | 256 | `CC_SWITCH_REMOTE_RELAY_MAX_RESERVATIONS` | 全服同时存活的中继预约总数（≈ 可挂载的在线客户端数） |
| `max_reservations_per_peer` | 2 | `CC_SWITCH_REMOTE_RELAY_MAX_RESERVATIONS_PER_PEER` | 单 Peer 预约数 |
| `reservation_duration` | 3600s | `CC_SWITCH_REMOTE_RELAY_RESERVATION_DURATION_SECS` | 预约有效期，到期未续约自动释放 |
| `max_circuits` | 128 | `CC_SWITCH_REMOTE_RELAY_MAX_CIRCUITS` | 全服同时存活的中继电路总数 |
| `max_circuits_per_peer` | 8 | `CC_SWITCH_REMOTE_RELAY_MAX_CIRCUITS_PER_PEER` | 单 Peer 作为端点的电路数 |
| `max_circuit_duration` | 600s | `CC_SWITCH_REMOTE_RELAY_MAX_CIRCUIT_DURATION_SECS` | 单条电路最长存活时间，到点强制关闭 |
| `max_circuit_bytes` | 1 GiB | `CC_SWITCH_REMOTE_RELAY_MAX_CIRCUIT_BYTES` | 单条电路最大传输字节，耗尽强制关闭 |

**单连接平均带宽 ≈ `max_circuit_bytes / max_circuit_duration`**，默认 1 GiB / 10 min ≈ **1.75 MB/s**。
需要提高/压低单路带宽时，按比例同向调整这两个值即可（例如 2 MB/s：`max_circuit_bytes=1258291200`（1200 MiB）+ `max_circuit_duration=600`）。

此外还有两层内置防护（来自 libp2p 默认配置，本服务器保留）：

- **速率限制器**：每 Peer 每 2 分钟最多 30 次预约/建路请求，每 IP 每分钟最多 60 次；
- **rendezvous 上限**：单 Peer 最多 32 条注册、全服最多 10000 条注册、TTL 范围 2h–72h（libp2p 服务端默认值）；

其他安全说明：

- 服务以非 root 用户运行，systemd unit 开启 `NoNewPrivileges` / `ProtectSystem=strict` 等加固；
- `relay.key` 为服务器唯一敏感文件，0600 权限；泄漏仅影响 relay 身份（可被冒充），不影响客户端间端到端加密内容；
- relay 不校验命名空间语义，任何知道地址的 libp2p 节点都可注册——资源上限即防护手段；如需私有 relay，可在防火墙层限制来源 IP，或自行 fork 增加鉴权 behaviour。

## 项目结构

```
├── Cargo.toml                  # libp2p 0.56.0 + tokio
├── src/
│   ├── main.rs                 # Swarm 装配、事件循环、启动横幅、优雅退出
│   ├── config.rs               # 环境变量配置与 relay 资源限制
│   └── keypair.rs              # Ed25519 密钥加载/生成（0600 持久化）
├── deploy/
│   ├── cc-switch-remote-relay.service  # systemd unit（Restart=always，非 root）
│   └── deploy.sh               # 一键部署脚本
├── LICENSE                     # MIT
└── README.md
```

## 许可证

[MIT](LICENSE)
