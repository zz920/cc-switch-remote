//! TokenTap Share 组网模块
//!
//! 多个客户端通过 share id 组网，把本地请求路由到网络内其他成员的机器上，
//! 消费对方共享出来的供应商额度。请求永远由出借方本机向上游发出（见
//! `docs/share-network-implementation-zh.md` 3.4 出站身份模型）。
//!
//! 模块划分：
//! - `swarm`：libp2p 事件循环（发现/打洞/加入审批）
//! - `bridge`：消费侧 loopback 桥接（远端节点以普通供应商形态进入路由）
//! - `ingress`：出借侧数据面（门禁 + 出口净化 + 受限路由）
//! - `route_hook`：注入现有 ProviderRouter 的组网路由钩子
//! - `auth` / `quota` / `identity` / `keystore`：鉴权/限额/身份/密钥存储

pub mod auth;
pub mod bridge;
pub mod config;
pub mod identity;
pub mod ingress;
pub mod keystore;
pub mod official;
pub mod quota;
pub mod route_hook;
pub mod swarm;
#[cfg(test)]
mod tests_e2e;
pub mod types;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use http::HeaderMap;
use libp2p::{Multiaddr, PeerId};
use serde_json::json;
use tauri::Emitter;
use tokio::sync::{oneshot, RwLock as AsyncRwLock};

use crate::database::Database;
use crate::database::ShareNetworkRow;
use crate::database::SHARE_REMOTE_PROVIDER_PREFIX;

use bridge::BridgeHandle;
use ingress::AdmissionCtx;
use route_hook::{
    remote_target_id, synthetic_remote_provider_for_provider, ConsumerRouteHook, RemotePeerRoute,
    RemoteRouteTable,
};
use swarm::{JoinRequestWire, JoinResponseWire, SwarmCmd, SwarmEventOut, SwarmHandle};
use types::*;

const ROUTE_TARGETS_SETTING: &str = "share_route_targets";
const SHARE_MODE_SETTING: &str = "share_mode";
/// 节点显示名的全局设置键：跨网络/重启保留（用户改过或自动生成过的名字）
const SHARE_NODE_NAME_SETTING: &str = "share_node_name";

/// 组网管理器（全局单例，存于 AppState）
#[derive(Clone)]
pub struct ShareManager {
    inner: Arc<ShareManagerInner>,
}

pub struct ShareManagerInner {
    db: Arc<Database>,
    /// Official 共享只从本机托管账号取凭据，不读取消费方认证头。
    codex_oauth_manager: Arc<crate::proxy::providers::codex_oauth_auth::CodexOAuthManager>,
    /// 节点身份密钥对（首次启动生成）
    identity: AsyncRwLock<Option<libp2p::identity::Keypair>>,
    /// 当前 share key（b64；准入校验共享读取，重生成即全员失效）
    share_key: Arc<AsyncRwLock<Option<String>>>,
    /// 当前网络配置
    network: AsyncRwLock<Option<ShareNetworkRow>>,
    swarm: AsyncRwLock<Option<SwarmHandle>>,
    bridge: AsyncRwLock<Option<BridgeHandle>>,
    /// 消费侧路由快照（ConsumerRouteHook 同步读取）
    route_table: Arc<RwLock<RemoteRouteTable>>,
    /// 出借白名单（"app:provider_id"）
    whitelist: Arc<RwLock<HashSet<String>>>,
    /// 限额配置（准入共享读取）
    quota: Arc<AsyncRwLock<ShareQuotaConfig>>,
    /// 节点显示名
    node_name: AsyncRwLock<String>,
    /// 当前配置的 relay 服务 PeerId（relay 不属于共享网络成员）
    relay_peer_id: AsyncRwLock<Option<String>>,
    /// relay 控制连接/预约状态，供连接诊断页展示
    relay_connected: AsyncRwLock<bool>,
    relay_transport: AsyncRwLock<Option<String>>,
    /// 已知节点（含离线，key = peer_id base58）
    peers: Arc<AsyncRwLock<HashMap<String, PeerRuntime>>>,
    /// 新节点连上时唤醒 join_wait_loop，让加入申请秒级送达而不必等轮询周期
    join_wake: Arc<tokio::sync::Notify>,
    /// 消费侧按应用选择的远端 Provider target（缺少 app 键表示使用本地 Provider）
    route_targets: AsyncRwLock<HashMap<String, HashSet<String>>>,
    /// 加入申请（出借方待审批，key = peer_id）
    join_requests: Arc<AsyncRwLock<HashMap<String, JoinRequestInfo>>>,
    join_responders: AsyncRwLock<HashMap<String, oneshot::Sender<JoinResponseWire>>>,
    /// 加入方待审批状态
    pending_join: AsyncRwLock<Option<PendingJoinState>>,
    /// 出借侧受限 ProxyState 缓存（按 peer）
    lend_states: AsyncRwLock<HashMap<String, Arc<crate::proxy::server::ProxyState>>>,
    proxy_service: AsyncRwLock<Option<crate::services::ProxyService>>,
    app_handle: Arc<AsyncRwLock<Option<tauri::AppHandle>>>,
    key_storage: AsyncRwLock<keystore::KeyStorage>,
    /// 防止事件消费/meta 任务随 leave 泄漏
    runtime_generation: AsyncRwLock<u64>,
}

#[derive(Debug, Clone)]
struct PeerRuntime {
    name: String,
    online: bool,
    /// 仅在使用当前共享密钥成功完成 meta 认证后置为 true。
    /// transport 连接本身不代表节点已经获准加入网络。
    authenticated: bool,
    direct: bool,
    shared_apps: Vec<String>,
    providers: Vec<ShareProviderInfo>,
}

fn apply_identified_name(peer: &mut PeerRuntime, name: String) {
    if !name.is_empty() && !peer.authenticated {
        peer.name = name;
    }
}

#[derive(Debug, Clone)]
struct PendingJoinState {
    share_id: String,
    short_code: String,
    expires_at: i64,
}

impl ShareManager {
    pub fn new(
        db: Arc<Database>,
        codex_oauth_manager: Arc<crate::proxy::providers::codex_oauth_auth::CodexOAuthManager>,
    ) -> Self {
        Self {
            inner: Arc::new(ShareManagerInner {
                db,
                codex_oauth_manager,
                identity: AsyncRwLock::new(None),
                share_key: Arc::new(AsyncRwLock::new(None)),
                network: AsyncRwLock::new(None),
                swarm: AsyncRwLock::new(None),
                bridge: AsyncRwLock::new(None),
                route_table: Arc::new(RwLock::new(RemoteRouteTable {
                    preference: RoutePreference::LocalOnly,
                    bridge_port: config::SHARE_BRIDGE_PORT,
                    remotes: Vec::new(),
                    selected_targets: HashMap::new(),
                })),
                whitelist: Arc::new(RwLock::new(HashSet::new())),
                quota: Arc::new(AsyncRwLock::new(ShareQuotaConfig {
                    scope: "daily".to_string(),
                    max_tokens: 0,
                    per_peer: true,
                })),
                node_name: AsyncRwLock::new(String::new()),
                relay_peer_id: AsyncRwLock::new(None),
                relay_connected: AsyncRwLock::new(false),
                relay_transport: AsyncRwLock::new(None),
                peers: Arc::new(AsyncRwLock::new(HashMap::new())),
                join_wake: Arc::new(tokio::sync::Notify::new()),
                route_targets: AsyncRwLock::new(HashMap::new()),
                join_requests: Arc::new(AsyncRwLock::new(HashMap::new())),
                join_responders: AsyncRwLock::new(HashMap::new()),
                pending_join: AsyncRwLock::new(None),
                lend_states: AsyncRwLock::new(HashMap::new()),
                proxy_service: AsyncRwLock::new(None),
                app_handle: Arc::new(AsyncRwLock::new(None)),
                key_storage: AsyncRwLock::new(keystore::KeyStorage::Keyring),
                runtime_generation: AsyncRwLock::new(0),
            }),
        }
    }

    /// 接线：注入 ProxyService 与 AppHandle（lib.rs setup 调用）
    pub async fn attach(
        &self,
        proxy_service: crate::services::ProxyService,
        app: tauri::AppHandle,
    ) {
        *self.inner.proxy_service.write().await = Some(proxy_service);
        *self.inner.app_handle.write().await = Some(app);
    }

    /// 启动恢复：若数据库中已有网络配置，加载 key 并恢复完整运行时
    pub async fn restore_from_db(&self) -> Result<(), String> {
        let Some(row) = self
            .inner
            .db
            .get_share_network()
            .map_err(|e| format!("读取组网配置失败: {e}"))?
        else {
            return Ok(());
        };
        let key = keystore::load_share_key(&row.share_id_hash)?
            .ok_or_else(|| "share key 缺失，请重新加入网络".to_string())?;
        log::info!("[Share] 恢复网络「{}」", row.share_id);
        self.apply_network_row(row.clone()).await;
        self.load_route_targets().await;
        *self.inner.share_key.write().await = Some(key);
        *self.inner.key_storage.write().await = keystore::current_storage(&row.share_id_hash);
        self.start_full_runtime().await
    }

    /// 把 DB 行同步到内存（白名单/限额/偏好/节点名）
    async fn apply_network_row(&self, row: ShareNetworkRow) {
        let mode = self.share_mode_for_row(&row).await;
        {
            let whitelist = if mode == ShareMode::Provider {
                row.shared_provider_ids.iter().cloned().collect()
            } else {
                HashSet::new()
            };
            *self
                .inner
                .whitelist
                .write()
                .unwrap_or_else(|e| e.into_inner()) = whitelist;
        }
        *self.inner.quota.write().await = ShareQuotaConfig {
            scope: row.quota_scope.clone(),
            max_tokens: row.quota_max_tokens,
            per_peer: row.quota_per_peer,
        };
        *self.inner.node_name.write().await = row.node_name.clone();
        {
            let mut table = self
                .inner
                .route_table
                .write()
                .unwrap_or_else(|e| e.into_inner());
            table.preference = if mode == ShareMode::Provider {
                RoutePreference::LocalOnly
            } else {
                self.route_preference_for_row(&row)
            };
        }
        *self.inner.network.write().await = Some(row);
    }

    /// 本机 PeerId（未启动时返回空串）
    pub async fn local_peer_id(&self) -> String {
        match self.inner.identity.read().await.as_ref() {
            Some(kp) => identity::peer_id_string(kp),
            None => String::new(),
        }
    }

    async fn ensure_identity(&self) -> Result<libp2p::identity::Keypair, String> {
        if let Some(kp) = self.inner.identity.read().await.as_ref() {
            return Ok(kp.clone());
        }
        let kp = identity::load_or_create_identity()?;
        *self.inner.identity.write().await = Some(kp.clone());
        Ok(kp)
    }

    /// 确保 swarm 运行（未运行则以当前节点名启动）
    async fn ensure_swarm(&self) -> Result<(), String> {
        if self.inner.swarm.read().await.is_some() {
            return Ok(());
        }
        let keypair = self.ensure_identity().await?;
        let node_name = self.inner.node_name.read().await.clone();
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<SwarmEventOut>(256);
        let handle = swarm::start_swarm(
            keypair,
            swarm::TransportKind::Real,
            node_name,
            event_tx,
            Vec::new(),
        )?;
        *self.inner.swarm.write().await = Some(handle);

        // 事件消费任务
        let generation = {
            let mut g = self.inner.runtime_generation.write().await;
            *g += 1;
            *g
        };
        let manager = self.clone();
        tokio::spawn(async move {
            manager.consume_events(event_rx, generation).await;
        });
        Ok(())
    }

    /// 配置 swarm 的网络参数（命名空间 + relay）
    async fn configure_swarm_network(&self, row: &ShareNetworkRow) -> Result<(), String> {
        let relay_addrs = resolve_relay_addrs(row.relay_addr.as_deref())?;
        if relay_addrs.is_empty() {
            return Err(
                "未配置 relay 地址：请在设置中填写 relay 地址（或等待官方 relay 上线）".to_string(),
            );
        }
        let namespace = format!(
            "{}:{}",
            config::RENDEZVOUS_NAMESPACE_PREFIX,
            row.share_id_hash
        );
        *self.inner.relay_peer_id.write().await = relay_addrs
            .iter()
            .find_map(extract_peer_id)
            .map(|peer| peer.to_base58());
        *self.inner.relay_connected.write().await = false;
        *self.inner.relay_transport.write().await = None;
        let swarm = self.inner.swarm.read().await;
        let Some(swarm) = swarm.as_ref() else {
            return Err("swarm 未运行".to_string());
        };
        swarm
            .cmd
            .send(SwarmCmd::Configure {
                namespace: Some(namespace),
                relay_addrs,
            })
            .await
            .map_err(|e| format!("配置 swarm 失败: {e}"))
    }

    /// 启动完整运行时（swarm + 桥接 + 路由钩子 + meta 刷新）
    async fn start_full_runtime(&self) -> Result<(), String> {
        self.ensure_swarm().await?;
        let row = self.inner.network.read().await.clone();
        let Some(row) = row else {
            return Err("未加入网络".to_string());
        };
        self.configure_swarm_network(&row).await?;

        // 消费侧桥接
        let bridge_port = {
            let mut bridge = self.inner.bridge.write().await;
            if let Some(b) = bridge.as_ref() {
                b.port()
            } else {
                let handle = bridge::start_bridge(self.clone(), config::SHARE_BRIDGE_PORT).await?;
                let port = handle.port();
                *bridge = Some(handle);
                port
            }
        };
        {
            let mut table = self
                .inner
                .route_table
                .write()
                .unwrap_or_else(|e| e.into_inner());
            table.bridge_port = bridge_port;
        }

        // 安装消费侧路由钩子
        if let Some(proxy) = self.inner.proxy_service.read().await.as_ref() {
            proxy
                .set_route_hook(Some(Arc::new(ConsumerRouteHook::new(
                    self.inner.route_table.clone(),
                ))))
                .await;
        }

        // meta 刷新任务
        let generation = *self.inner.runtime_generation.read().await;
        let manager = self.clone();
        tokio::spawn(async move {
            manager.meta_refresh_loop(generation).await;
        });

        self.sync_route_table().await;
        Ok(())
    }

    /// 停止运行时（保留/清除网络配置由调用方决定）
    async fn stop_runtime(&self) {
        {
            let mut g = self.inner.runtime_generation.write().await;
            *g += 1; // 使事件/meta 任务自行退出
        }
        if let Some(bridge) = self.inner.bridge.write().await.take() {
            bridge.stop().await;
        }
        if let Some(swarm) = self.inner.swarm.write().await.take() {
            let _ = swarm.cmd.send(SwarmCmd::Shutdown).await;
        }
        if let Some(proxy) = self.inner.proxy_service.read().await.as_ref() {
            proxy.set_route_hook(None).await;
        }
        {
            let mut table = self
                .inner
                .route_table
                .write()
                .unwrap_or_else(|e| e.into_inner());
            *table = RemoteRouteTable {
                preference: RoutePreference::LocalOnly,
                bridge_port: config::SHARE_BRIDGE_PORT,
                remotes: Vec::new(),
                selected_targets: HashMap::new(),
            };
        }
        self.inner.peers.write().await.clear();
        *self.inner.relay_peer_id.write().await = None;
        *self.inner.relay_connected.write().await = false;
        *self.inner.relay_transport.write().await = None;
        self.inner.join_requests.write().await.clear();
        self.inner.join_responders.write().await.clear();
        self.inner.lend_states.write().await.clear();
    }

    // ==================== 网络生命周期 ====================

    /// 创建网络（出借方/创建者）
    pub async fn create_network(&self, node_name: String) -> Result<CreateNetworkResult, String> {
        if self.inner.network.read().await.is_some() {
            return Err("已在网络中，请先退出当前网络".to_string());
        }
        let share_id = auth::generate_share_id();
        let share_key = auth::generate_share_key();
        let hash = auth::share_id_hash(&share_id);
        let storage = keystore::save_share_key(&hash, &share_key)?;
        *self.inner.key_storage.write().await = storage;

        let now = chrono::Utc::now().timestamp();
        let row = ShareNetworkRow {
            share_id: share_id.clone(),
            share_id_hash: hash,
            role: ShareRole::Creator.as_str().to_string(),
            // 加入网络不等于同意立即把 API 流量交给网络节点；由用户在 UI
            // 中显式开启共享供应商路由。
            route_preference: RoutePreference::LocalOnly.as_str().to_string(),
            relay_addr: None,
            shared_provider_ids: Vec::new(),
            quota_scope: "daily".to_string(),
            quota_max_tokens: 0,
            quota_per_peer: true,
            node_name,
            created_at: now,
            updated_at: now,
        };
        self.inner
            .db
            .save_share_network(&row)
            .map_err(|e| e.to_string())?;
        self.inner
            .db
            .set_setting(SHARE_MODE_SETTING, ShareMode::Provider.as_str())
            .map_err(|e| e.to_string())?;
        self.apply_network_row(row).await;
        *self.inner.share_key.write().await = Some(share_key.clone());
        self.start_full_runtime().await?;

        Ok(CreateNetworkResult {
            share_id,
            share_key,
        })
    }

    /// 发起加入（消费方）：生成短码，等待出借方审批
    pub async fn request_join(
        &self,
        share_id_input: String,
        relay_addr: Option<String>,
    ) -> Result<RequestJoinResult, String> {
        if self.inner.network.read().await.is_some() {
            return Err("已在网络中，请先退出当前网络".to_string());
        }
        if self.inner.pending_join.read().await.is_some() {
            return Err("已有加入申请在等待审批".to_string());
        }
        let share_id = auth::normalize_share_id(&share_id_input);
        if share_id.len() != 8 {
            return Err("share id 格式不正确（8 位字符）".to_string());
        }
        let hash = auth::share_id_hash(&share_id);

        // 加入方尚无网络记录，因此必须先把对话框中提供的 Relay 保存为全局配置。
        // 后续 request_join 与获批后的正式网络运行都会读取同一份配置。
        if let Some(relay_addr) = relay_addr.filter(|value| !value.trim().is_empty()) {
            self.set_relay_addr(Some(relay_addr)).await?;
        }

        // 以待审批网络配置启动 swarm（仅用于发现；审批通过前无 key，无法消费）
        self.ensure_swarm().await?;
        let short_code = auth::generate_short_code();
        let expires_at = chrono::Utc::now().timestamp() + config::JOIN_REQUEST_TTL_SECS;

        let namespace = format!("{}:{}", config::RENDEZVOUS_NAMESPACE_PREFIX, hash);
        let relay_addrs = resolve_relay_addrs(None)?;
        if relay_addrs.is_empty() {
            return Err(
                "未配置 relay 地址：请在设置中填写 relay 地址（或等待官方 relay 上线）".to_string(),
            );
        }
        *self.inner.relay_peer_id.write().await = relay_addrs
            .iter()
            .find_map(extract_peer_id)
            .map(|peer| peer.to_base58());
        *self.inner.relay_connected.write().await = false;
        *self.inner.relay_transport.write().await = None;
        {
            let swarm = self.inner.swarm.read().await;
            let swarm = swarm.as_ref().expect("swarm just started");
            swarm
                .cmd
                .send(SwarmCmd::Configure {
                    namespace: Some(namespace),
                    relay_addrs,
                })
                .await
                .map_err(|e| format!("配置 swarm 失败: {e}"))?;
        }

        let pending = PendingJoinState {
            share_id: share_id.clone(),
            short_code: short_code.clone(),
            expires_at,
        };
        *self.inner.pending_join.write().await = Some(pending);

        // 后台审批等待任务
        let manager = self.clone();
        let code = short_code.clone();
        tokio::spawn(async move {
            manager
                .join_wait_loop(share_id, hash, code, expires_at)
                .await;
        });

        Ok(RequestJoinResult {
            short_code,
            expires_at,
        })
    }

    /// 加入等待循环：向在线节点广播加入申请直到获批/超时
    async fn join_wait_loop(
        &self,
        share_id: String,
        hash: String,
        short_code: String,
        expires_at: i64,
    ) {
        let node_name = self.resolve_node_name().await;
        loop {
            // 已被取消（用户退出/超时清理）
            if self.inner.pending_join.read().await.is_none() {
                return;
            }
            if chrono::Utc::now().timestamp() >= expires_at {
                *self.inner.pending_join.write().await = None;
                self.emit(
                    "share:join-resolved",
                    json!({"accepted": false, "reason": "审批超时"}),
                );
                return;
            }

            let peers: Vec<String> = {
                let peers = self.inner.peers.read().await;
                let relay_peer_id = self.inner.relay_peer_id.read().await.clone();
                peers
                    .iter()
                    .filter(|(id, p)| p.online && relay_peer_id.as_deref() != Some(id.as_str()))
                    .map(|(id, _)| id.clone())
                    .collect()
            };

            for peer_id in peers {
                let Ok(peer) = peer_id.parse::<PeerId>() else {
                    continue;
                };
                let (tx, rx) = oneshot::channel();
                let cmd = {
                    let swarm = self.inner.swarm.read().await;
                    swarm.as_ref().map(|s| s.cmd.clone())
                };
                let Some(cmd) = cmd else { return };
                let _ = cmd
                    .send(SwarmCmd::SendJoinRequest {
                        peer,
                        request: JoinRequestWire {
                            short_code: short_code.clone(),
                            node_name: node_name.clone(),
                        },
                        respond: tx,
                    })
                    .await;

                // 审批是人工操作，必须让响应通道存活到本次申请到期。
                // 原先固定 15 秒会让 consumer 丢弃仍在 provider 等待审批的流，
                // 随后的批准只能写回失效流，consumer 因而永远无法完成加入。
                let response_timeout = join_response_timeout(expires_at);
                if let Ok(Ok(Ok(response))) = tokio::time::timeout(response_timeout, rx).await {
                    if response.accepted {
                        if let Some(key) = response.share_key {
                            self.finalize_join(&share_id, &hash, key).await;
                        }
                        return;
                    }
                }
            }

            // 有新节点连上时立即重试（PeerConnected 触发 notify_waiters），
            // sleep 仅作兜底，避免申请送达多等一个轮询周期。
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {}
                _ = self.inner.join_wake.notified() => {}
            }
        }
    }

    /// 审批通过后的收尾：存 key、落库、启动完整运行时
    async fn finalize_join(&self, share_id: &str, hash: &str, share_key: String) {
        let result: Result<(), String> = async {
            let storage = keystore::save_share_key(hash, &share_key)?;
            *self.inner.key_storage.write().await = storage;
            let now = chrono::Utc::now().timestamp();
            let display_id = format!(
                "{}-{}",
                &share_id[..4.min(share_id.len())],
                &share_id[4.min(share_id.len())..]
            );
            let row = ShareNetworkRow {
                share_id: display_id,
                share_id_hash: hash.to_string(),
                role: ShareRole::Member.as_str().to_string(),
                // 审批通过后保持本地路由，等待用户显式开启共享供应商。
                route_preference: RoutePreference::LocalOnly.as_str().to_string(),
                relay_addr: None,
                shared_provider_ids: Vec::new(),
                quota_scope: "daily".to_string(),
                quota_max_tokens: 0,
                quota_per_peer: true,
                node_name: self.resolve_node_name().await,
                created_at: now,
                updated_at: now,
            };
            self.inner
                .db
                .save_share_network(&row)
                .map_err(|e| e.to_string())?;
            // 加入方默认以用户（消费方）身份入网：仅本机路由，等用户显式
            // 切换共享供应商。创建方才默认供应商（见 create_network）。
            self.inner
                .db
                .set_setting(SHARE_MODE_SETTING, ShareMode::Consumer.as_str())
                .map_err(|e| e.to_string())?;
            *self.inner.pending_join.write().await = None;
            self.apply_network_row(row).await;
            *self.inner.share_key.write().await = Some(share_key);
            self.start_full_runtime().await?;
            // key 刚到位：立即对在线节点做一次 meta 认证刷新，别等 UI 轮询
            // 或 60s 兜底循环（否则节点数会空转半分钟以上才显示对端）。
            self.refresh_online_peer_meta().await;
            Ok(())
        }
        .await;

        match result {
            Ok(()) => {
                self.emit("share:join-resolved", json!({"accepted": true}));
                log::info!("[Share] 已加入网络");
            }
            Err(e) => {
                log::error!("[Share] 加入网络失败: {e}");
                self.emit(
                    "share:join-resolved",
                    json!({"accepted": false, "reason": e}),
                );
            }
        }
    }

    /// 取消等待中的加入申请
    pub async fn cancel_join(&self) -> Result<(), String> {
        if self.inner.pending_join.write().await.take().is_none() {
            return Err("没有等待中的加入申请".to_string());
        }
        Ok(())
    }

    /// 出借方审批加入申请
    pub async fn approve_join(&self, peer_id: &str) -> Result<(), String> {
        if !self.is_creator().await {
            return Err("只有网络创建者可以审批加入申请".to_string());
        }
        let responder = self.inner.join_responders.write().await.remove(peer_id);
        let Some(responder) = responder else {
            return Err("加入申请不存在或已过期".to_string());
        };
        let key = self
            .inner
            .share_key
            .read()
            .await
            .clone()
            .ok_or_else(|| "网络密钥不可用".to_string())?;
        let _ = responder.send(JoinResponseWire {
            accepted: true,
            share_key: Some(key),
            reason: None,
        });
        self.inner.join_requests.write().await.remove(peer_id);
        self.emit(
            "share:join-resolved",
            json!({"peerId": peer_id, "accepted": true}),
        );
        // 审批响应发出后立即尝试认证，节点无需等待下一轮周期刷新；
        // 若对端尚未完成落盘，周期任务仍会继续重试。
        let manager = self.clone();
        let pid = peer_id.to_string();
        tokio::spawn(async move {
            manager.refresh_peer_meta(&pid).await;
        });
        Ok(())
    }

    /// 出借方拒绝加入申请
    pub async fn reject_join(&self, peer_id: &str, reason: Option<String>) -> Result<(), String> {
        if !self.is_creator().await {
            return Err("只有网络创建者可以处理加入申请".to_string());
        }
        let responder = self.inner.join_responders.write().await.remove(peer_id);
        let Some(responder) = responder else {
            return Err("加入申请不存在或已过期".to_string());
        };
        let _ = responder.send(JoinResponseWire {
            accepted: false,
            share_key: None,
            reason,
        });
        self.inner.join_requests.write().await.remove(peer_id);
        self.emit(
            "share:join-resolved",
            json!({"peerId": peer_id, "accepted": false}),
        );
        Ok(())
    }

    /// 退出/解散网络
    pub async fn leave_network(&self) -> Result<(), String> {
        let row = self.inner.network.read().await.clone();
        let shared_apps = self
            .inner
            .route_targets
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        self.stop_runtime().await;
        *self.inner.network.write().await = None;
        *self.inner.share_key.write().await = None;
        self.inner.route_targets.write().await.clear();
        let _ = self.persist_route_targets().await;
        self.sync_route_table().await;
        if let Err(error) = self
            .restore_local_provider_live_for_apps(&shared_apps)
            .await
        {
            log::error!("退出共享网络后恢复本地 Provider 配置失败: {error}");
        }
        if let Some(row) = row {
            let _ = keystore::delete_share_key(&row.share_id_hash);
        }
        self.inner
            .db
            .delete_share_network()
            .map_err(|e| e.to_string())?;
        self.inner
            .db
            .set_setting(SHARE_MODE_SETTING, ShareMode::Provider.as_str())
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    // ==================== 出借方控制 ====================

    /// 设置出借白名单（"app:provider_id" 列表）
    pub async fn set_shared_providers(&self, ids: Vec<String>) -> Result<(), String> {
        if self.current_share_mode().await == ShareMode::Consumer {
            return Err("当前节点是用户模式，不能配置共享 Provider".to_string());
        }
        // Official 仅允许出借显式绑定且已加载的本机 Codex OAuth 账号。
        let mut cleaned = Vec::new();
        for entry in ids {
            let Some((app, provider_id)) = entry.split_once(':') else {
                return Err(format!("无效的白名单条目: {entry}"));
            };
            let provider = self
                .inner
                .db
                .get_provider_by_id(provider_id, app)
                .map_err(|e| format!("读取供应商失败: {e}"))?
                .ok_or_else(|| format!("供应商不存在: {entry}"))?;
            if provider.category.as_deref() == Some("official") {
                let account_id = official::managed_codex_official_account_id(app, &provider)
                    .ok_or_else(|| {
                        format!(
                            "Official Provider 仅支持显式绑定本机 Codex OAuth 账号: {}",
                            provider.name
                        )
                    })?;
                if !self
                    .inner
                    .codex_oauth_manager
                    .has_account(&account_id)
                    .await
                {
                    return Err(format!(
                        "Official Provider 绑定的 Codex OAuth 账号不可用，请重新登录: {}",
                        provider.name
                    ));
                }
            }
            cleaned.push(entry);
        }
        {
            *self
                .inner
                .whitelist
                .write()
                .unwrap_or_else(|e| e.into_inner()) = cleaned.iter().cloned().collect();
        }
        self.update_network_row(|row| row.shared_provider_ids = cleaned)
            .await
    }

    /// 设置限额
    pub async fn set_quota(&self, cfg: ShareQuotaConfig) -> Result<(), String> {
        if !matches!(cfg.scope.as_str(), "daily" | "monthly") {
            return Err("限额周期须为 daily 或 monthly".to_string());
        }
        *self.inner.quota.write().await = cfg.clone();
        self.update_network_row(|row| {
            row.quota_scope = cfg.scope;
            row.quota_max_tokens = cfg.max_tokens;
            row.quota_per_peer = cfg.per_peer;
        })
        .await
    }

    /// 拉黑节点（即时生效：准入检查每次请求都查黑名单）
    pub async fn block_peer(&self, peer_id: &str, reason: Option<String>) -> Result<(), String> {
        if self.local_peer_id().await == peer_id
            || self.inner.relay_peer_id.read().await.as_deref() == Some(peer_id)
        {
            return Err("不能拉黑本机或 relay 服务节点".to_string());
        }
        self.inner
            .db
            .block_share_peer(peer_id, reason.as_deref())
            .map_err(|e| e.to_string())?;
        // 移除出借状态缓存，切断后续服务
        self.inner.lend_states.write().await.remove(peer_id);
        {
            let mut peers = self.inner.peers.write().await;
            if let Some(p) = peers.get_mut(peer_id) {
                p.online = false;
            }
        }
        self.sync_route_table().await;
        Ok(())
    }

    /// 解除拉黑
    pub async fn unblock_peer(&self, peer_id: &str) -> Result<(), String> {
        self.inner
            .db
            .unblock_share_peer(peer_id)
            .map_err(|e| e.to_string())
    }

    /// 重新生成 share key（旧 key 立即失效，等效全员踢出）
    pub async fn regenerate_key(&self) -> Result<String, String> {
        let row = self.inner.network.read().await.clone();
        let Some(row) = row else {
            return Err("未加入网络".to_string());
        };
        let new_key = auth::generate_share_key();
        keystore::save_share_key(&row.share_id_hash, &new_key)?;
        *self.inner.share_key.write().await = Some(new_key.clone());
        log::info!("[Share] share key 已重新生成，旧 key 立即失效");
        Ok(new_key)
    }

    /// 设置共享网络路由偏好。旧版传入 provider/consumer 时仍视为角色切换。
    pub async fn set_route_preference(&self, preference: String) -> Result<(), String> {
        if matches!(preference.as_str(), "provider" | "consumer") {
            return self.set_route_mode(preference).await;
        }
        if !matches!(
            preference.as_str(),
            "local_only" | "local_first" | "network_first" | "network_only"
        ) {
            return Err("不支持的共享网络路由偏好".to_string());
        }
        let mode = self.current_share_mode().await;
        let pref = RoutePreference::from_str_lossy(&preference);
        if mode == ShareMode::Provider && pref != RoutePreference::LocalOnly {
            return Err("供应商模式只能使用本地路由".to_string());
        }
        {
            let mut table = self
                .inner
                .route_table
                .write()
                .unwrap_or_else(|e| e.into_inner());
            table.preference = pref;
        }
        self.update_network_row(|row| row.route_preference = pref.as_str().to_string())
            .await?;
        self.sync_route_table().await;
        Ok(())
    }

    /// 设置本节点角色（provider / consumer）。角色与路由偏好相互独立。
    pub async fn set_route_mode(&self, mode: String) -> Result<(), String> {
        if !matches!(mode.as_str(), "provider" | "consumer") {
            return Err("共享网络角色必须是 provider 或 consumer".to_string());
        }
        let mode = ShareMode::from_str_lossy(&mode);
        if mode == ShareMode::Provider {
            let previous = self.inner.route_targets.read().await.clone();
            let shared_apps = previous.keys().cloned().collect::<Vec<_>>();
            self.inner.route_targets.write().await.clear();
            if let Err(error) = self.persist_route_targets().await {
                *self.inner.route_targets.write().await = previous;
                return Err(error);
            }
            self.sync_route_table().await;
            // 共享 Codex/Claude/Grok 会把 live 中的模型字段投影成远端
            // Provider；切回供应商角色时必须同步回当前本地 Provider。
            self.restore_local_provider_live_for_apps(&shared_apps)
                .await?;
        }
        self.inner
            .db
            .set_setting(SHARE_MODE_SETTING, mode.as_str())
            .map_err(|e| e.to_string())?;
        self.update_network_row(|row| {
            if mode == ShareMode::Provider {
                row.route_preference = RoutePreference::LocalOnly.as_str().to_string();
            } else if matches!(row.route_preference.as_str(), "provider" | "consumer") {
                row.route_preference = RoutePreference::LocalOnly.as_str().to_string();
            }
        })
        .await?;
        self.sync_route_table().await;
        Ok(())
    }

    /// 设置某个应用实际参与共享路由的远端 Provider target。
    /// target 格式为 `<peer_id>:<provider_id>`，只保存选择，不保存任何密钥。
    pub async fn set_route_targets(
        &self,
        app_type: String,
        targets: Vec<String>,
    ) -> Result<(), String> {
        if self.current_share_mode().await != ShareMode::Consumer {
            return Err("当前节点是供应商模式，不能配置共享网络路由".to_string());
        }
        if !matches!(
            app_type.as_str(),
            "claude" | "codex" | "gemini" | "grokbuild"
        ) {
            return Err(format!("不支持的共享路由应用: {app_type}"));
        }
        let cleaned: HashSet<String> = targets
            .into_iter()
            .map(|target| target.trim().to_string())
            .filter(|target| !target.is_empty())
            .collect();
        let previous = self.inner.route_targets.read().await.clone();
        {
            let mut route_targets = self.inner.route_targets.write().await;
            if cleaned.is_empty() {
                route_targets.remove(&app_type);
            } else {
                route_targets.insert(app_type, cleaned);
            }
        }
        if let Err(error) = self.persist_route_targets().await {
            *self.inner.route_targets.write().await = previous;
            self.sync_route_table().await;
            return Err(error);
        }
        self.sync_route_table().await;
        Ok(())
    }

    /// 启用一个共享 Provider，并把 Agent live 配置投影到本地代理。
    ///
    /// 这不能拆成前端的「先接管、再选 target」两个调用：Codex 的 live
    /// `model` / `model_provider` 必须来自所选远端 Provider，而不是当前本地
    /// Provider，否则界面显示已选中但 Codex 启动后会直接请求错误模型。
    pub async fn activate_shared_provider(
        &self,
        app_type: String,
        peer_id: String,
        provider_id: String,
    ) -> Result<(), String> {
        if self.current_share_mode().await != ShareMode::Consumer {
            return Err("当前节点是供应商模式，不能启用共享 Provider".to_string());
        }
        if !matches!(
            app_type.as_str(),
            "claude" | "codex" | "gemini" | "grokbuild"
        ) {
            return Err(format!("不支持的共享路由应用: {app_type}"));
        }

        let (peer_name, advertised) = {
            let peers = self.inner.peers.read().await;
            let peer = peers
                .get(&peer_id)
                .filter(|peer| peer.online && peer.authenticated)
                .ok_or_else(|| "共享节点当前不可用".to_string())?;
            let provider = peer
                .providers
                .iter()
                .find(|provider| provider.app == app_type && provider.provider_id == provider_id)
                .cloned()
                .ok_or_else(|| "该 Provider 尚未在节点能力信息中出现".to_string())?;
            (peer.name.clone(), provider)
        };
        if app_type == "codex"
            // 托管 OAuth（OpenAI Official）的模型由账号动态决定，公告可能不带
            // 模型；此时合成 config.toml 不写 model 字段，交给 Codex CLI 自身的
            // 默认模型，不能因此拒绝激活。
            && advertised.auth_mode.as_deref() != Some("managed_oauth")
            && advertised
                .default_model
                .as_deref()
                .or_else(|| advertised.models.first().map(String::as_str))
                .is_none()
        {
            return Err(
                "该共享 Codex Provider 没有通告默认模型，请先在提供方保存模型配置并刷新网络"
                    .to_string(),
            );
        }

        let bridge_port = self
            .inner
            .route_table
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .bridge_port;
        let synthetic = synthetic_remote_provider_for_provider(
            &app_type,
            &peer_id,
            &peer_name,
            &advertised,
            bridge_port,
        );
        let proxy = self
            .inner
            .proxy_service
            .read()
            .await
            .clone()
            .ok_or_else(|| "本地路由服务尚未初始化".to_string())?;
        let takeover = proxy.get_takeover_status().await?;
        let takeover_was_active = match app_type.as_str() {
            "claude" => takeover.claude,
            "codex" => takeover.codex,
            "gemini" => takeover.gemini,
            "grokbuild" => takeover.grokbuild,
            _ => false,
        };
        let codex_before = if app_type == "codex" {
            Some(
                crate::codex_config::CodexLiveStateSnapshot::capture()
                    .map_err(|error| format!("捕获 Codex 配置状态失败: {error}"))?,
            )
        } else {
            None
        };

        proxy.set_takeover_for_app(&app_type, true).await?;
        let live_result = match app_type.as_str() {
            "codex" => {
                proxy
                    .sync_codex_live_from_provider_while_proxy_active(&synthetic)
                    .await
            }
            "claude" => {
                proxy
                    .sync_claude_live_from_provider_while_proxy_active(&synthetic)
                    .await
            }
            "grokbuild" => {
                proxy
                    .sync_grok_live_from_provider_while_proxy_active(&synthetic)
                    .await
            }
            // Gemini 接管只需要固定本地代理地址；实际 Provider 由路由 target 决定。
            "gemini" => Ok(()),
            _ => unreachable!(),
        };
        if let Err(error) = live_result {
            self.rollback_shared_activation(
                &proxy,
                &app_type,
                takeover_was_active,
                codex_before.as_ref(),
            )
            .await;
            return Err(error);
        }

        if let Err(error) = self
            .set_route_targets(
                app_type.clone(),
                vec![remote_target_id(&peer_id, &provider_id)],
            )
            .await
        {
            self.rollback_shared_activation(
                &proxy,
                &app_type,
                takeover_was_active,
                codex_before.as_ref(),
            )
            .await;
            return Err(error);
        }
        Ok(())
    }

    async fn rollback_shared_activation(
        &self,
        proxy: &crate::services::ProxyService,
        app_type: &str,
        takeover_was_active: bool,
        codex_before: Option<&crate::codex_config::CodexLiveStateSnapshot>,
    ) {
        let rollback = if !takeover_was_active {
            proxy.set_takeover_for_app(app_type, false).await
        } else if let Some(snapshot) = codex_before {
            snapshot
                .restore_preserving_newer_same_account_auth()
                .map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        if let Err(error) = rollback {
            log::error!("共享 Provider 启用失败后恢复 {app_type} 配置失败: {error}");
        }
    }

    async fn restore_local_provider_live_for_apps(
        &self,
        app_types: &[String],
    ) -> Result<(), String> {
        if app_types.is_empty() {
            return Ok(());
        }
        let Some(proxy) = self.inner.proxy_service.read().await.clone() else {
            return Err("本地路由服务尚未初始化".to_string());
        };
        let takeover = proxy.get_takeover_status().await?;

        for app_type in app_types {
            let is_active = match app_type.as_str() {
                "claude" => takeover.claude,
                "codex" => takeover.codex,
                "gemini" => takeover.gemini,
                "grokbuild" => takeover.grokbuild,
                _ => false,
            };
            if !is_active || app_type == "gemini" {
                continue;
            }

            let app = crate::app_config::AppType::from_str(app_type)
                .map_err(|error| format!("无效的应用类型 {app_type}: {error}"))?;
            let provider_id = crate::settings::get_effective_current_provider(&self.inner.db, &app)
                .map_err(|error| format!("读取 {app_type} 当前本地 Provider 失败: {error}"))?;
            let provider = match provider_id {
                Some(provider_id) => self
                    .inner
                    .db
                    .get_provider_by_id(&provider_id, app_type)
                    .map_err(|error| format!("读取 {app_type} 本地 Provider 失败: {error}"))?,
                None => None,
            };
            let Some(provider) = provider else {
                // 没有可投影的本地 Provider 时恢复 Agent 原生配置，避免保留
                // tokentap_shared model_provider 和占位认证信息。
                proxy.set_takeover_for_app(app_type, false).await?;
                continue;
            };

            match app_type.as_str() {
                "codex" => {
                    proxy
                        .sync_codex_live_from_provider_while_proxy_active(&provider)
                        .await?
                }
                "claude" => {
                    proxy
                        .sync_claude_live_from_provider_while_proxy_active(&provider)
                        .await?
                }
                "grokbuild" => {
                    proxy
                        .sync_grok_live_from_provider_while_proxy_active(&provider)
                        .await?
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn load_route_targets(&self) {
        let parsed = self
            .inner
            .db
            .get_setting(ROUTE_TARGETS_SETTING)
            .ok()
            .flatten()
            .and_then(|raw| serde_json::from_str::<HashMap<String, HashSet<String>>>(&raw).ok())
            .unwrap_or_default();
        *self.inner.route_targets.write().await = parsed;
    }

    async fn persist_route_targets(&self) -> Result<(), String> {
        let snapshot = self.route_targets_snapshot().await;
        let json = serde_json::to_string(&snapshot).map_err(|e| e.to_string())?;
        self.inner
            .db
            .set_setting(ROUTE_TARGETS_SETTING, &json)
            .map_err(|e| e.to_string())
    }

    async fn route_targets_snapshot(&self) -> HashMap<String, Vec<String>> {
        let targets = self.inner.route_targets.read().await;
        targets
            .iter()
            .map(|(app, values)| {
                let mut values = values.iter().cloned().collect::<Vec<_>>();
                values.sort();
                (app.clone(), values)
            })
            .collect()
    }

    /// 设置 relay 地址覆盖
    pub async fn set_relay_addr(&self, addr: Option<String>) -> Result<(), String> {
        let normalized = normalize_relay_addr_list(addr.as_deref())?;
        let persisted = normalized.as_deref().map(str::to_string);
        std::fs::create_dir_all(config::share_data_dir()).map_err(|e| e.to_string())?;
        let path = config::relay_config_path();
        match persisted.as_deref() {
            Some(value) => std::fs::write(&path, value).map_err(|e| e.to_string())?,
            None => {
                if path.exists() {
                    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                }
            }
        }
        if self.inner.network.read().await.is_some() {
            self.update_network_row(|row| row.relay_addr = persisted.clone())
                .await?;
        }
        // 热更新 swarm 网络配置
        if let Some(row) = self.inner.network.read().await.clone() {
            if self.inner.swarm.read().await.is_some() {
                self.configure_swarm_network(&row).await?;
            }
        }
        Ok(())
    }

    /// 设置节点显示名
    pub async fn set_node_name(&self, name: String) -> Result<(), String> {
        *self.inner.node_name.write().await = name.clone();
        // 同步落全局设置：退出网络/重启后仍保留用户选择
        if let Err(e) = self.inner.db.set_setting(SHARE_NODE_NAME_SETTING, &name) {
            log::warn!("[Share] 保存节点显示名失败: {e}");
        }
        self.update_network_row(|row| row.node_name = name).await
    }

    /// 解析节点显示名：已设置直接返回；否则读全局设置，仍无则生成随机名
    /// 并持久化。生成后落库，保证跨重启/跨请求稳定，用户可随时改。
    pub async fn resolve_node_name(&self) -> String {
        {
            let name = self.inner.node_name.read().await;
            if !name.trim().is_empty() {
                return name.clone();
            }
        }
        let stored = self
            .inner
            .db
            .get_setting(SHARE_NODE_NAME_SETTING)
            .unwrap_or(None)
            .filter(|s| !s.trim().is_empty());
        let name = stored.unwrap_or_else(generate_random_node_name);
        if let Err(e) = self.inner.db.set_setting(SHARE_NODE_NAME_SETTING, &name) {
            log::warn!("[Share] 保存随机节点名失败: {e}");
        }
        *self.inner.node_name.write().await = name.clone();
        name
    }

    async fn update_network_row(&self, f: impl FnOnce(&mut ShareNetworkRow)) -> Result<(), String> {
        let mut guard = self.inner.network.write().await;
        let Some(row) = guard.as_mut() else {
            return Err("未加入网络".to_string());
        };
        f(row);
        row.updated_at = chrono::Utc::now().timestamp();
        let snapshot = row.clone();
        drop(guard);
        self.inner
            .db
            .save_share_network(&snapshot)
            .map_err(|e| e.to_string())?;
        self.apply_network_row(snapshot).await;
        Ok(())
    }

    async fn is_creator(&self) -> bool {
        self.inner
            .network
            .read()
            .await
            .as_ref()
            .map(|row| ShareRole::from_str_lossy(&row.role) == ShareRole::Creator)
            .unwrap_or(false)
    }

    async fn share_mode_for_row(&self, row: &ShareNetworkRow) -> ShareMode {
        self.inner
            .db
            .get_setting(SHARE_MODE_SETTING)
            .ok()
            .flatten()
            .map(|value| ShareMode::from_str_lossy(&value))
            .unwrap_or_else(|| ShareMode::from_str_lossy(&row.route_preference))
    }

    async fn current_share_mode(&self) -> ShareMode {
        let Some(row) = self.inner.network.read().await.clone() else {
            return ShareMode::Provider;
        };
        self.share_mode_for_row(&row).await
    }

    fn route_preference_for_row(&self, row: &ShareNetworkRow) -> RoutePreference {
        match row.route_preference.as_str() {
            "provider" => RoutePreference::LocalOnly,
            "consumer" => RoutePreference::NetworkOnly,
            value => RoutePreference::from_str_lossy(value),
        }
    }

    // ==================== 状态 ====================

    /// 完整组网状态（share_get_status）
    pub async fn get_status(&self) -> Result<ShareNetworkStatus, String> {
        // 节点名称通过 meta 通告动态同步；状态查询是前端的固定轮询入口，
        // 在这里主动刷新一次可避免远端必须等待完整的后台刷新周期。
        self.refresh_online_peer_meta().await;
        let row = self.inner.network.read().await.clone();
        let pending = self.inner.pending_join.read().await.clone();
        let requests: Vec<JoinRequestInfo> = self
            .inner
            .join_requests
            .read()
            .await
            .values()
            .cloned()
            .collect();
        let bridge_running = self.inner.bridge.read().await.is_some();
        let local_peer_id = self.local_peer_id().await;
        let relay_connected = *self.inner.relay_connected.read().await;
        let relay_transport = self.inner.relay_transport.read().await.clone();

        let (scope, max_tokens) = {
            let quota = self.inner.quota.read().await;
            (quota.scope.clone(), quota.max_tokens)
        };
        let usage_since = quota::period_start_epoch(&scope);
        let provided_tokens = self
            .inner
            .db
            .share_all_peers_tokens_used(usage_since)
            .unwrap_or_default()
            .into_iter()
            .map(|(_, tokens)| tokens)
            .sum();
        let consumed_tokens = self
            .inner
            .db
            .share_consumed_tokens(usage_since)
            .unwrap_or(0);

        let mut peers_out: Vec<SharePeerInfo> = Vec::new();
        let blocked = self.inner.db.list_share_blocked_peers().unwrap_or_default();
        let blocked_set: HashSet<String> = blocked.iter().map(|b| b.peer_id.clone()).collect();
        let relay_peer_id = self.inner.relay_peer_id.read().await.clone();
        // 路由卡逐行用量：本周期经「share:<peer>:<provider_id>」消费的 token。
        // 一次查询，按完整 provider_id 建索引。
        let provider_used: HashMap<String, i64> = self
            .inner
            .db
            .share_remote_provider_tokens_used(usage_since)
            .unwrap_or_default()
            .into_iter()
            .collect();

        {
            let peers = self.inner.peers.read().await;
            for (peer_id, p) in peers.iter() {
                if relay_peer_id.as_deref() == Some(peer_id.as_str()) {
                    continue;
                }
                // 连接建立并不等于审批通过。申请中的节点需要先完成共享密钥
                // 认证，避免在节点管理和消费路由中提前暴露。
                if !p.authenticated {
                    continue;
                }
                let (tokens_used, quota_remaining) =
                    quota::peer_quota_status(&self.inner.db, peer_id, &scope, max_tokens)
                        .unwrap_or((0, None));
                let mut providers = p.providers.clone();
                for provider in providers.iter_mut() {
                    let key = provider_usage_key(peer_id, &provider.provider_id);
                    if let Some(used) = provider_used.get(&key) {
                        provider.used_tokens = Some(*used);
                    }
                }
                peers_out.push(SharePeerInfo {
                    peer_id: peer_id.clone(),
                    name: p.name.clone(),
                    online: p.online,
                    direct: p.direct,
                    shared_apps: p.shared_apps.clone(),
                    providers,
                    tokens_used,
                    quota_remaining,
                    is_blocked: blocked_set.contains(peer_id),
                });
            }
        }
        // 黑名单中不在线列表里的节点也展示
        for b in blocked {
            if relay_peer_id.as_deref() == Some(b.peer_id.as_str()) {
                continue;
            }
            if !peers_out.iter().any(|p| p.peer_id == b.peer_id) {
                peers_out.push(SharePeerInfo {
                    peer_id: b.peer_id,
                    name: String::new(),
                    online: false,
                    direct: false,
                    shared_apps: Vec::new(),
                    providers: Vec::new(),
                    tokens_used: 0,
                    quota_remaining: None,
                    is_blocked: true,
                });
            }
        }

        let key_storage = *self.inner.key_storage.read().await;
        let _ = key_storage; // 供前端风险提示（随 status 之外单独查询亦可）

        Ok(match row {
            Some(row) => {
                let mode = self.share_mode_for_row(&row).await;
                ShareNetworkStatus {
                    joined: true,
                    share_id: Some(row.share_id),
                    role: Some(row.role.clone()),
                    route_preference: self
                        .inner
                        .route_table
                        .read()
                        .map(|table| table.preference.as_str().to_string())
                        .unwrap_or_else(|_| RoutePreference::LocalOnly.as_str().to_string()),
                    mode: mode.as_str().to_string(),
                    route_targets: self.route_targets_snapshot().await,
                    node_name: row.node_name,
                    relay_addr: row.relay_addr.or_else(|| load_global_relay_addr()),
                    shared_provider_ids: row.shared_provider_ids,
                    quota_scope: row.quota_scope,
                    quota_max_tokens: row.quota_max_tokens,
                    quota_per_peer: row.quota_per_peer,
                    provided_tokens,
                    consumed_tokens,
                    peers: peers_out,
                    pending_join: pending.map(|p| PendingJoinInfo {
                        share_id: p.share_id,
                        short_code: p.short_code,
                        expires_at: p.expires_at,
                    }),
                    incoming_requests: requests,
                    bridge_running,
                    relay_connected,
                    relay_transport,
                    local_peer_id,
                }
            }
            None => ShareNetworkStatus {
                joined: false,
                relay_addr: load_global_relay_addr(),
                peers: peers_out,
                pending_join: pending.map(|p| PendingJoinInfo {
                    share_id: p.share_id,
                    short_code: p.short_code,
                    expires_at: p.expires_at,
                }),
                incoming_requests: requests,
                bridge_running,
                relay_connected,
                relay_transport,
                local_peer_id,
                ..Default::default()
            },
        })
    }

    /// key 存储位置（UI 风险提示）
    pub async fn key_storage(&self) -> keystore::KeyStorage {
        *self.inner.key_storage.read().await
    }

    // ==================== 内部：事件与路由 ====================

    /// 消费 swarm 事件
    async fn consume_events(
        &self,
        mut rx: tokio::sync::mpsc::Receiver<SwarmEventOut>,
        generation: u64,
    ) {
        loop {
            if *self.inner.runtime_generation.read().await != generation {
                return;
            }
            let Some(event) = rx.recv().await else { return };
            match event {
                SwarmEventOut::PeerConnected { peer_id, direct } => {
                    if self.inner.relay_peer_id.read().await.as_deref()
                        == Some(peer_id.to_base58().as_str())
                    {
                        continue;
                    }
                    {
                        let mut peers = self.inner.peers.write().await;
                        let entry = peers.entry(peer_id.to_base58()).or_insert(PeerRuntime {
                            name: String::new(),
                            online: false,
                            authenticated: false,
                            direct: false,
                            shared_apps: Vec::new(),
                            providers: Vec::new(),
                        });
                        entry.online = true;
                        // 每次建立新的 transport 连接都重新确认共享密钥，避免旧
                        // 会话的认证状态在密钥轮换或重新入网后被沿用。
                        entry.authenticated = false;
                        entry.direct = direct;
                    }
                    self.sync_route_table().await;
                    // 唤醒可能在等轮询周期的加入申请循环，立即向新节点发申请
                    self.inner.join_wake.notify_waiters();
                    // 连接建立后立即拉一次能力通告
                    let manager = self.clone();
                    let pid = peer_id.to_base58();
                    tokio::spawn(async move {
                        manager.refresh_peer_meta(&pid).await;
                    });
                }
                SwarmEventOut::PeerDisconnected { peer_id } => {
                    if self.inner.relay_peer_id.read().await.as_deref()
                        == Some(peer_id.to_base58().as_str())
                    {
                        continue;
                    }
                    {
                        let mut peers = self.inner.peers.write().await;
                        if let Some(p) = peers.get_mut(&peer_id.to_base58()) {
                            p.online = false;
                            p.direct = false;
                        }
                    }
                    self.sync_route_table().await;
                }
                SwarmEventOut::PeerIdentified { peer_id, name } => {
                    if self.inner.relay_peer_id.read().await.as_deref()
                        == Some(peer_id.to_base58().as_str())
                    {
                        continue;
                    }
                    let mut peers = self.inner.peers.write().await;
                    let entry = peers.entry(peer_id.to_base58()).or_insert(PeerRuntime {
                        name: String::new(),
                        online: false,
                        authenticated: false,
                        direct: false,
                        shared_apps: Vec::new(),
                        providers: Vec::new(),
                    });
                    // Identify 的 agent_version 是 swarm 启动时的快照，只能作为
                    // 未认证节点的临时名称；已认证节点以共享 meta 中的名称为准，
                    // 防止旧 Identify 事件覆盖用户后来修改的节点名。
                    apply_identified_name(entry, name);
                }
                SwarmEventOut::HttpInbound { peer_id, stream } => {
                    let manager = self.clone();
                    tokio::spawn(async move {
                        manager.serve_inbound(peer_id, stream).await;
                    });
                }
                SwarmEventOut::JoinInbound {
                    peer_id,
                    request,
                    respond,
                } => {
                    let pid = peer_id.to_base58();
                    if !self.is_creator().await {
                        let _ = respond.send(JoinResponseWire {
                            accepted: false,
                            share_key: None,
                            reason: Some("只有网络创建者可以审批加入申请".to_string()),
                        });
                        continue;
                    }
                    let duplicate = self
                        .inner
                        .join_requests
                        .read()
                        .await
                        .get(&pid)
                        .map(|existing| existing.short_code == request.short_code)
                        .unwrap_or(false);
                    if duplicate {
                        // 对端可能因连接闪断重发同一申请。保留原申请卡，但把
                        // 审批响应切换到最新的存活流，避免批准写入旧连接。
                        self.inner
                            .join_responders
                            .write()
                            .await
                            .insert(pid, respond);
                        continue;
                    }
                    let info = JoinRequestInfo {
                        peer_id: pid.clone(),
                        node_name: request.node_name,
                        short_code: request.short_code,
                        received_at: chrono::Utc::now().timestamp(),
                    };
                    self.inner
                        .join_requests
                        .write()
                        .await
                        .insert(pid.clone(), info.clone());
                    self.inner
                        .join_responders
                        .write()
                        .await
                        .insert(pid.clone(), respond);
                    let request_code = info.short_code.clone();
                    self.emit("share:join-request", info);
                    // 超时自动拒绝
                    let manager = self.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(
                            config::JOIN_REQUEST_TTL_SECS as u64,
                        ))
                        .await;
                        let is_same_request = manager
                            .inner
                            .join_requests
                            .read()
                            .await
                            .get(&pid)
                            .map(|request| request.short_code == request_code)
                            .unwrap_or(false);
                        if is_same_request {
                            let _ = manager
                                .reject_join(&pid, Some("审批超时".to_string()))
                                .await;
                        }
                    });
                }
                SwarmEventOut::RelayState {
                    connected,
                    transport,
                } => {
                    *self.inner.relay_connected.write().await = connected;
                    *self.inner.relay_transport.write().await = transport;
                    log::info!(
                        "[Share] relay 状态: {}",
                        if connected { "已连接" } else { "断开" }
                    );
                }
            }
        }
    }

    /// 出借侧服务入站数据面流
    async fn serve_inbound(&self, peer_id: PeerId, stream: libp2p::Stream) {
        let pid = peer_id.to_base58();
        let app_handle = self.inner.app_handle.read().await.clone();
        let state = {
            let mut states = self.inner.lend_states.write().await;
            match states.get(&pid) {
                Some(s) => s.clone(),
                None => {
                    let s = Arc::new(
                        ingress::build_lend_state(
                            self.inner.db.clone(),
                            &pid,
                            self.inner.whitelist.clone(),
                            app_handle,
                        )
                        .await,
                    );
                    states.insert(pid.clone(), s.clone());
                    s
                }
            }
        };
        let ctx = AdmissionCtx::new(
            self.inner.db.clone(),
            pid,
            self.inner.share_key.clone(),
            self.inner.quota.clone(),
            self.inner.whitelist.clone(),
            self.inner.node_name.read().await.clone(),
            self.inner.codex_oauth_manager.clone(),
        );
        ingress::serve_lend_stream(stream, (*state).clone(), ctx).await;
    }

    /// 同步消费侧路由快照（online + 未拉黑节点）
    async fn sync_route_table(&self) {
        let selected_targets = self.inner.route_targets.read().await.clone();
        let peers = self.inner.peers.read().await;
        let relay_peer_id = self.inner.relay_peer_id.read().await.clone();
        let remotes: Vec<RemotePeerRoute> = peers
            .iter()
            .filter(|(id, p)| {
                p.online && p.authenticated && relay_peer_id.as_deref() != Some(id.as_str())
            })
            .map(|(id, p)| RemotePeerRoute {
                peer_id: id.clone(),
                name: p.name.clone(),
                shared_apps: p.shared_apps.clone(),
                providers: p.providers.clone(),
            })
            .collect();
        drop(peers);
        let mut table = self
            .inner
            .route_table
            .write()
            .unwrap_or_else(|e| e.into_inner());
        table.remotes = remotes;
        table.selected_targets = selected_targets;
    }

    /// 主动刷新当前在线且已认证节点的能力通告。
    ///
    /// 状态查询由前端定期调用，因此将刷新放在该入口可以让节点名称等
    /// 元数据在下一次轮询内同步，同时限制并发请求的单次等待时间。
    async fn refresh_online_peer_meta(&self) {
        // 注意不能按 authenticated 过滤：authenticated 只有 meta 刷新成功才会置
        // true，过滤未认证节点会让"审批后拿到 key 的节点"永远等 60s 兜底循环
        // 才完成首次认证（表现为节点数 0 持续半分钟以上）。在线即刷新。
        let peer_ids: Vec<String> = {
            let peers = self.inner.peers.read().await;
            let relay_peer_id = self.inner.relay_peer_id.read().await.clone();
            peers
                .iter()
                .filter(|(id, p)| p.online && relay_peer_id.as_deref() != Some(id.as_str()))
                .map(|(id, _)| id.clone())
                .collect()
        };
        let manager = self.clone();
        let tasks = peer_ids.into_iter().map(|peer_id| {
            let manager = manager.clone();
            async move {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    manager.refresh_peer_meta(&peer_id),
                )
                .await;
            }
        });
        futures::future::join_all(tasks).await;
    }

    /// meta 周期刷新
    async fn meta_refresh_loop(&self, generation: u64) {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(config::META_REFRESH_SECS)).await;
            if *self.inner.runtime_generation.read().await != generation {
                return;
            }
            let peers: Vec<String> = {
                let peers = self.inner.peers.read().await;
                let relay_peer_id = self.inner.relay_peer_id.read().await.clone();
                peers
                    .iter()
                    .filter(|(id, p)| p.online && relay_peer_id.as_deref() != Some(id.as_str()))
                    .map(|(id, _)| id.clone())
                    .collect()
            };
            for pid in peers {
                self.refresh_peer_meta(&pid).await;
            }
        }
    }

    /// 查询单个节点的能力通告并更新路由表
    async fn refresh_peer_meta(&self, peer_id: &str) {
        if self.inner.relay_peer_id.read().await.as_deref() == Some(peer_id) {
            return;
        }
        if self.inner.share_key.read().await.is_none() {
            return;
        }
        let result = self
            .http_via_peer(
                peer_id,
                http::Method::GET,
                config::META_PATH,
                HeaderMap::new(),
                Vec::new(),
            )
            .await;
        let response = match result {
            Ok(response) => response,
            Err(error) => {
                // 静默失败会让节点永远停留在未认证状态（UI 节点数为 0）且无从排查，
                // 必须留下线索。典型原因：对端离线、流协议握手失败。
                log::warn!("[Share] 拉取节点 {peer_id} meta 失败: {error}");
                return;
            }
        };
        // 只有成功的 meta 响应才证明对端接受了当前共享密钥。不能仅凭
        // “请求有 HTTP 响应”判定认证成功，否则 401 错误体也可能被误记为在线。
        if !response.status().is_success() {
            log::warn!(
                "[Share] 节点 {peer_id} meta 响应 HTTP {}（共享密钥不匹配，或本机与对端时钟偏差超过 ±{}s 防重放窗口）",
                response.status().as_u16(),
                config::AUTH_WINDOW_SECS
            );
            return;
        }
        let body = match axum::body::to_bytes(response.into_body(), 64 * 1024).await {
            Ok(b) => b,
            Err(_) => return,
        };
        let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&body) else {
            return;
        };
        let shared_apps: Vec<String> = meta
            .get("sharedApps")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let name = meta
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        {
            let mut peers = self.inner.peers.write().await;
            let entry = peers.entry(peer_id.to_string()).or_insert(PeerRuntime {
                name: String::new(),
                online: true,
                authenticated: false,
                direct: false,
                shared_apps: Vec::new(),
                providers: Vec::new(),
            });
            entry.authenticated = true;
            entry.online = true;
            entry.shared_apps = shared_apps;
            entry.providers =
                serde_json::from_value(meta.get("providers").cloned().unwrap_or_else(|| json!([])))
                    .unwrap_or_default();
            if !name.is_empty() {
                entry.name = name;
            }
        }
        self.sync_route_table().await;
    }

    /// 消费侧数据面：经 P2P 向节点发起一个 HTTP 请求
    pub async fn http_via_peer(
        &self,
        peer_id: &str,
        method: http::Method,
        path_and_query: &str,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> Result<axum::response::Response, String> {
        let key = self
            .inner
            .share_key
            .read()
            .await
            .clone()
            .ok_or_else(|| "未加入网络".to_string())?;
        let peer: PeerId = peer_id.parse().map_err(|e| format!("无效的节点 id: {e}"))?;
        let control = {
            let swarm = self.inner.swarm.read().await;
            swarm.as_ref().map(|s| s.stream_control.clone())
        };
        let mut control = control.ok_or_else(|| "组网未运行".to_string())?;

        let stream = control
            .open_stream(peer, config::DATA_PLANE_PROTOCOL)
            .await
            .map_err(|e| format!("无法连接节点: {e:?}"))?;

        let path_only = path_and_query.split('?').next().unwrap_or(path_and_query);
        let body_hash = auth::body_sha256_hex(&body);
        let ts = chrono::Utc::now().timestamp();
        let nonce = auth::generate_nonce();
        let auth_header =
            auth::sign_request(&key, ts, &nonce, method.as_str(), path_only, &body_hash)?;

        let mut builder = http::Request::builder().method(method).uri(path_and_query);
        for (name, value) in headers.iter() {
            let n = name.as_str();
            if n == "host" || n == "content-length" || n == "connection" {
                continue;
            }
            builder = builder.header(name, value);
        }
        let request = builder
            .header(config::HEADER_AUTH, auth_header)
            .body(http_body_util::Full::new(bytes::Bytes::from(body)))
            .map_err(|e| format!("构建请求失败: {e}"))?;

        let io = hyper_util::rt::TokioIo::new(
            tokio_util::compat::FuturesAsyncReadCompatExt::compat(stream),
        );
        let (mut sender, conn) = hyper::client::conn::http1::handshake(io)
            .await
            .map_err(|e| format!("建立连接失败: {e}"))?;
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let response = sender
            .send_request(request)
            .await
            .map_err(|e| format!("请求节点失败: {e}"))?;

        let (parts, body) = response.into_parts();
        let mut out = axum::response::Response::new(axum::body::Body::new(body));
        *out.status_mut() = parts.status;
        *out.headers_mut() = parts.headers;
        Ok(out)
    }

    /// 请求出借方检查某个共享 Provider 的连通性。
    /// 该请求只执行健康检查，不提交模型请求，也不消耗共享配额。
    pub async fn test_remote_provider(
        &self,
        app_type: String,
        peer_id: String,
        provider_id: String,
    ) -> Result<ShareProviderCheckResult, String> {
        let peer = {
            let peers = self.inner.peers.read().await;
            peers
                .get(&peer_id)
                .filter(|peer| peer.online && peer.authenticated && !peer.providers.is_empty())
                .cloned()
                .ok_or_else(|| "共享节点当前不可用".to_string())?
        };
        let advertised = peer
            .providers
            .iter()
            .any(|provider| provider.app == app_type && provider.provider_id == provider_id);
        if !advertised {
            return Err("该 Provider 尚未在节点能力信息中出现".to_string());
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            config::HEADER_ROUTE_PROVIDER,
            http::HeaderValue::from_str(&provider_id)
                .map_err(|e| format!("Provider target 无效: {e}"))?,
        );
        let response = self
            .http_via_peer(
                &peer_id,
                http::Method::GET,
                &format!("{}?app={}", config::PROVIDER_CHECK_PATH, app_type),
                headers,
                Vec::new(),
            )
            .await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .map_err(|e| format!("读取连通性结果失败: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "出借方检查接口返回 HTTP {}: {}",
                status,
                String::from_utf8_lossy(&body)
            ));
        }
        serde_json::from_slice::<ShareProviderCheckResult>(&body)
            .map_err(|e| format!("解析连通性结果失败: {e}"))
    }

    fn emit<T: serde::Serialize + Clone + Send + 'static>(&self, event: &str, payload: T) {
        let app_handle = self.inner.app_handle.clone();
        let event = event.to_string();
        tokio::spawn(async move {
            if let Some(app) = app_handle.read().await.as_ref() {
                if let Err(e) = app.emit(&event, payload) {
                    log::debug!("[Share] 事件发送失败 {event}: {e}");
                }
            }
        });
    }
}

/// 解析 relay 地址（网络覆盖 > 已保存配置 > 环境变量 > 官方默认）。
fn resolve_relay_addrs(override_addr: Option<&str>) -> Result<Vec<Multiaddr>, String> {
    let mut out = Vec::new();
    match override_addr {
        Some(addr) if !addr.trim().is_empty() => {
            out.extend(parse_relay_addr_list(addr, "relay 地址")?);
        }
        _ => {
            if let Some(global_addr) = load_global_relay_addr() {
                out.extend(parse_relay_addr_list(&global_addr, "已保存的 relay 地址")?);
            }
            // 环境变量（运维/测试注入）：逗号/换行分隔
            if out.is_empty() {
                if let Ok(env_addrs) = std::env::var("TOKENTAP_RELAY_ADDRS") {
                    out.extend(parse_relay_addr_list(
                        &env_addrs,
                        "环境变量中的 relay 地址",
                    )?);
                }
            }
            if out.is_empty() {
                for addr in config::DEFAULT_RELAY_ADDRS {
                    out.push(
                        addr.parse::<Multiaddr>()
                            .map_err(|e| format!("内置 relay 地址无效: {e}"))?,
                    );
                }
            }
        }
    }
    out.sort_by_key(|addr| {
        if addr
            .iter()
            .any(|protocol| matches!(protocol, libp2p::multiaddr::Protocol::QuicV1))
        {
            0
        } else {
            1
        }
    });
    out.dedup();
    Ok(out)
}

fn parse_relay_addr_list(value: &str, source: &str) -> Result<Vec<Multiaddr>, String> {
    let parsed = value
        .split([',', '\n', '\r'])
        .map(str::trim)
        .filter(|addr| !addr.is_empty())
        .map(|addr| {
            addr.parse::<Multiaddr>()
                .map_err(|error| format!("{source}无效: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut addresses = Vec::with_capacity(parsed.len() * 2);
    for addr in parsed {
        if let Some(quic) = quic_variant(&addr) {
            addresses.push(quic);
        }
        addresses.push(addr);
    }
    Ok(addresses)
}

fn quic_variant(addr: &Multiaddr) -> Option<Multiaddr> {
    if addr
        .iter()
        .any(|protocol| matches!(protocol, libp2p::multiaddr::Protocol::QuicV1))
        || !addr
            .iter()
            .any(|protocol| matches!(protocol, libp2p::multiaddr::Protocol::Tcp(_)))
        || addr
            .iter()
            .any(|protocol| matches!(protocol, libp2p::multiaddr::Protocol::P2pCircuit))
    {
        return None;
    }
    let raw = addr.to_string();
    let (prefix, suffix) = raw.split_once("/tcp/")?;
    let (port, tail) = suffix.split_once('/')?;
    Some(format!("{prefix}/udp/{port}/quic-v1/{tail}").parse().ok()?)
}

fn normalize_relay_addr_list(value: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let mut addresses = parse_relay_addr_list(value, "relay 地址")?;
    addresses.sort_by_key(|addr| {
        if addr
            .iter()
            .any(|protocol| matches!(protocol, libp2p::multiaddr::Protocol::QuicV1))
        {
            0
        } else {
            1
        }
    });
    addresses.dedup();
    if addresses.is_empty() {
        Ok(None)
    } else {
        Ok(Some(
            addresses
                .into_iter()
                .map(|addr| addr.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ))
    }
}

/// 随机节点名字母表：大写字母 + 数字，剔除易混淆字符（I/O/0/1）。
const NODE_NAME_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// 生成 16 位随机节点显示名（大写字母 + 数字，如 `7KQ2M3XVA9BC4DEF`）。
pub fn generate_random_node_name() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| {
            let index = rng.gen_range(0..NODE_NAME_ALPHABET.len());
            NODE_NAME_ALPHABET[index] as char
        })
        .collect()
}

/// 消费侧请求日志的 provider_id 形态：`share:<peer_id>:<provider_id>`。
/// 必须与 DAO 聚合（share_remote_provider_tokens_used 的 LIKE 前缀）严格一致，
/// 抽成函数供两侧共用并有单测对齐（曾因少一个冒号导致统计恒为 0）。
fn provider_usage_key(peer_id: &str, provider_id: &str) -> String {
    format!("{SHARE_REMOTE_PROVIDER_PREFIX}:{peer_id}:{provider_id}")
}

fn load_global_relay_addr() -> Option<String> {
    std::fs::read_to_string(config::relay_config_path())
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn extract_peer_id(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|protocol| match protocol {
        libp2p::multiaddr::Protocol::P2p(peer_id) => Some(peer_id),
        _ => None,
    })
}

fn join_response_timeout(expires_at: i64) -> std::time::Duration {
    let remaining = expires_at.saturating_sub(chrono::Utc::now().timestamp());
    std::time::Duration::from_secs(remaining.max(1) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_node_name_shape_and_variety() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..32 {
            let name = generate_random_node_name();
            // 16 位、大写字母 + 数字（剔除易混淆字符）
            assert_eq!(name.len(), 16, "16 chars: {name}");
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                "uppercase alnum only: {name}"
            );
            assert!(!name.contains('I') && !name.contains('O'));
            assert!(!name.contains('0') && !name.contains('1'));
            seen.insert(name);
        }
        // 32 次抽样互不重复（32^16 空间，随机性健全性）
        assert_eq!(seen.len(), 32);
    }

    #[tokio::test]
    async fn resolve_node_name_generates_persists_and_honors_override() {
        let temp = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::memory().unwrap());
        let oauth = Arc::new(
            crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
                temp.path().to_path_buf(),
            ),
        );
        let manager = ShareManager::new(db.clone(), oauth);

        // 初始为空 → 自动生成并持久化
        let first = manager.resolve_node_name().await;
        assert!(!first.trim().is_empty());
        // 第二次解析返回同一名字（内存缓存）
        assert_eq!(manager.resolve_node_name().await, first);
        // 设置里已落库
        assert_eq!(
            db.get_setting(SHARE_NODE_NAME_SETTING).unwrap().as_deref(),
            Some(first.as_str())
        );

        // 用户改名：生效且持久化（模拟重启后从设置读取）
        manager.set_node_name("我的节点".to_string()).await.ok();
        assert_eq!(manager.resolve_node_name().await, "我的节点");
        let temp2 = tempfile::tempdir().unwrap();
        let oauth2 = Arc::new(
            crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
                temp2.path().to_path_buf(),
            ),
        );
        let manager2 = ShareManager::new(db.clone(), oauth2);
        assert_eq!(manager2.resolve_node_name().await, "我的节点");
    }

    #[test]
    fn provider_usage_key_matches_dao_aggregation_shape() {
        // get_status 的查询键必须与 DAO 聚类的 LIKE 前缀形态一致：
        // share:<peer_id>:<provider_id>（曾因少一个冒号导致统计恒为 0）
        let key = provider_usage_key("12D3KooWabc", "zhipu-1");
        assert_eq!(key, "share:12D3KooWabc:zhipu-1");
        // 与 DAO 侧的 LIKE 前缀（share:%）按相同首段开始，避免两侧形态漂移
        assert!(key.starts_with(crate::database::SHARE_REMOTE_PROVIDER_PREFIX));
        assert_eq!(
            key.matches(':').count(),
            2,
            "exactly two separators: share:<peer>:<provider>"
        );
    }

    #[test]
    fn relay_config_prefers_quic_and_keeps_tcp_fallback() {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer = PeerId::from(keypair.public());
        let tcp = format!("/ip4/127.0.0.1/tcp/15720/p2p/{peer}");

        let normalized = normalize_relay_addr_list(Some(&tcp))
            .expect("valid relay address")
            .expect("non-empty relay address");
        let candidates = normalized.split(',').collect::<Vec<_>>();

        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].contains("/udp/15720/quic-v1/"));
        assert!(candidates[1].contains("/tcp/15720/"));
    }

    #[test]
    fn relay_config_accepts_multiple_addresses_and_deduplicates() {
        let keypair = libp2p::identity::Keypair::generate_ed25519();
        let peer = PeerId::from(keypair.public());
        let quic = format!("/ip4/127.0.0.1/udp/15720/quic-v1/p2p/{peer}");
        let tcp = format!("/ip4/127.0.0.1/tcp/15720/p2p/{peer}");
        let input = format!("{tcp}\n{quic}\n{tcp}");

        let normalized = normalize_relay_addr_list(Some(&input))
            .expect("valid relay addresses")
            .expect("non-empty relay addresses");
        let candidates = normalized.split(',').collect::<Vec<_>>();

        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].contains("/udp/15720/quic-v1/"));
        assert!(candidates[1].contains("/tcp/15720/"));
    }

    #[test]
    fn join_response_waits_for_the_remaining_approval_window() {
        let expires_at = chrono::Utc::now().timestamp() + config::JOIN_REQUEST_TTL_SECS;
        let timeout = join_response_timeout(expires_at);

        assert!(timeout >= std::time::Duration::from_secs(299));
        assert!(timeout <= std::time::Duration::from_secs(300));
    }

    #[test]
    fn identify_name_is_provisional_after_meta_authentication() {
        let mut peer = PeerRuntime {
            name: "用户设置的名称".to_string(),
            online: true,
            authenticated: true,
            direct: true,
            shared_apps: Vec::new(),
            providers: Vec::new(),
        };
        apply_identified_name(&mut peer, "启动时的旧名称".to_string());
        assert_eq!(peer.name, "用户设置的名称");

        peer.authenticated = false;
        apply_identified_name(&mut peer, "未认证节点名称".to_string());
        assert_eq!(peer.name, "未认证节点名称");
    }
}
