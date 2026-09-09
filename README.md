# cc-switch-remote

基于 [cc-switch](https://github.com/farion1231/cc-switch)（MIT License）二次开发。

**cc-switch-remote** 在 cc-switch 的供应商管理与本地路由能力之上，新增 **share id 组网**：
多个客户端通过 share id 组成 P2P 网络，把本地 Claude Code / Codex 等 CLI 的请求
路由到网络内其他成员的机器上，消费对方共享出来的供应商额度。

请求永远由**出借方本机**向上游服务商发出（出站身份模型）——服务商全程只看到
出借方的机器与凭据；消费方的 API Key 从不离开消费方本机。

```
消费方 CLI ──本机代理──► P2P 加密隧道（QUIC / IPv6 直连 / relay 中继）──► 出借方本机代理 ──► 上游服务商
```

## 组件

| 目录 | 说明 |
|---|---|
| `src-tauri/` | 桌面应用（Tauri 2 + React）：供应商管理、本地路由接管、组网、配额 |
| `relay/` | 信令/中继服务器（独立 crate）：rendezvous 服务端 + circuit relay v2，供自建 relay 用 |

桌面客户端**默认连接官方 relay**（`tokentap.top`），无需自建任何服务端即可组网。
`relay/` 面向需要自建信令基础设施的高级用户，构建与部署见 [relay/README.md](relay/README.md)。

## 组网工作方式

1. **创建网络**的一方获得一个 8 位 share id（如 `4JM8-4DXS`）
2. 其他成员凭 share id 申请加入，创建方审批
3. 网络成员选择性地把自己的供应商"出借"给网络（默认什么都不共享）
4. 消费方在本机代理里把某个 Agent 的路由切到网络成员的供应商上
5. 计费走出借方账号；出借方可按天/月设置 token 配额与每节点限额

**安全模型**（务必阅读）：

- 消费方的凭据（API Key / OAuth）**永不出境**，出借方收到的是净化后的请求
- 出借方注入自己的凭据向上游发起全新请求
- **信任边界**：出借方对其转发的请求/响应内容完全可见（这是"流量出借"的固有属性）。
  请只与你信任的人组网；涉及公司/客户代码的机器不要加入
- 传输层全程加密（QUIC-TLS / Noise）；relay 服务器无法解密端到端内容

## 构建

桌面端（要求 Node 20+、pnpm、Rust ≥ 1.83，Windows 需 VS C++ Build Tools）：

```bash
pnpm install --frozen-lockfile
pnpm tauri build --bundles nsis        # Windows (.exe)
pnpm tauri build --bundles deb,appimage  # Linux
pnpm tauri build --bundles dmg         # macOS
```

relay 服务端：

```bash
cd relay && cargo build --release
```

## 与 cc-switch 的关系

cc-switch-remote 感谢 [cc-switch](https://github.com/farion1231/cc-switch) 及其贡献者。
上游的供应商管理、MCP、Skills、多应用支持等能力完整保留；本仓库的增量改动集中
在 P2P 组网（`share/` 模块）与凭据存储安全加固。上游的完整功能介绍请移步其 README。

## 免责声明 / Disclaimer

本项目按 "AS IS" 提供，不附带任何明示或默示的保证。使用者需自行承担使用风险：

- **账号风险**：共享供应商额度可能违反上游服务商（OpenAI、Anthropic、智谱等）的
  服务条款，可能导致账号被封禁或限制。请在使用前阅读并遵守相关条款
- **数据暴露**：组网出借方对其转发的请求与响应内容完全可见（见上方安全模型），
  请勿在涉及敏感或机密信息的机器上使用共享功能
- **合规使用**：请确保你的使用方式符合所在司法辖区的法律法规

This project is provided "AS IS", without warranty of any kind. Sharing
provider quota may violate upstream providers' terms of service and lead
to account suspension. The lending party has full visibility into relayed
traffic. Use at your own risk and in compliance with your local laws.

## License

MIT（见 [LICENSE](LICENSE) 与 [NOTICE](NOTICE)）。
