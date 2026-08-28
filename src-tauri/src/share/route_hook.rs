//! 组网路由钩子：消费侧远端注入 / 出借侧白名单过滤
//!
//! 消费侧：把在线的远端节点合成为普通 Provider（base_url 指向本机桥接
//! `http://127.0.0.1:{bridge}/peer/{peer_id}`）。每个 Agent 通过是否设置
//! selected target 在本地 Provider 与共享 Provider 之间互斥切换，
//! 熔断/故障转移/协议转换全部复用现有 forwarder。
//!
//! 出借侧：把白名单内的本机供应商克隆为按 peer 归因的合成 Provider
//! （id = `sharelend:<peer>:<id>`），用量经 proxy_request_logs 落到该 peer 名下。

use crate::database::lend_provider_id;
use crate::provider::Provider;
use crate::proxy::provider_router::RouteHook;
use serde_json::json;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use toml_edit::{value, DocumentMut, Item, Table};

use super::types::{RoutePreference, ShareProviderInfo};
use std::collections::HashMap;

/// 消费侧远端路由快照（ShareManager 异步维护，钩子同步读取）
#[derive(Debug, Clone, Default)]
pub struct RemoteRouteTable {
    pub preference: RoutePreference,
    pub bridge_port: u16,
    /// 远端节点路由项
    pub remotes: Vec<RemotePeerRoute>,
    /// 按 app 保存的 target 选择。非空表示该 app 已启用共享 Provider；
    /// 缺少 app 键或空集合表示继续使用本地 Provider。
    pub selected_targets: HashMap<String, std::collections::HashSet<String>>,
}

/// 单个远端节点的路由信息
#[derive(Debug, Clone)]
pub struct RemotePeerRoute {
    pub peer_id: String,
    pub name: String,
    /// 该节点共享的应用类型
    pub shared_apps: Vec<String>,
    /// 节点实际共享的 Provider 摘要
    pub providers: Vec<ShareProviderInfo>,
}

impl RemotePeerRoute {
    fn supports(&self, app_type: &str) -> bool {
        self.shared_apps.iter().any(|a| a == app_type)
    }
}

/// 消费侧远端供应商 id 前缀
pub const SHARE_REMOTE_PROVIDER_PREFIX: &str = "share";

/// 消费侧远端供应商 id
pub fn remote_provider_id(peer_id: &str) -> String {
    format!("{SHARE_REMOTE_PROVIDER_PREFIX}:{peer_id}")
}

/// 远端 Provider 的合成 id（用于本地日志归因和熔断器隔离）。
pub fn remote_provider_id_for_provider(peer_id: &str, provider_id: &str) -> String {
    format!("{SHARE_REMOTE_PROVIDER_PREFIX}:{peer_id}:{provider_id}")
}

/// 远端路由 target（在本地配置中保存，不包含共享密钥）。
pub fn remote_target_id(peer_id: &str, provider_id: &str) -> String {
    format!("{peer_id}:{provider_id}")
}

/// 构造消费侧远端节点的合成 Provider
///
/// 以普通第三方供应商的形态出现，base_url 指向本机桥接；
/// auth 为占位符（真正的 key 在出借方本机注入，消费方从不持有）。
pub fn synthetic_remote_provider(
    app_type: &str,
    peer_id: &str,
    name: &str,
    bridge_port: u16,
) -> Provider {
    let base_url = format!("http://127.0.0.1:{bridge_port}/peer/{peer_id}");
    let settings_config = match app_type {
        "claude" => json!({
            "env": {
                "ANTHROPIC_BASE_URL": base_url,
                "ANTHROPIC_AUTH_TOKEN": "tokentap",
            }
        }),
        "codex" => json!({
            "auth": { "OPENAI_API_KEY": "tokentap" },
            "config": "",
            "base_url": base_url,
        }),
        "gemini" => json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": base_url,
                "GEMINI_API_KEY": "tokentap",
            }
        }),
        // 其余应用（grokbuild/opencode 等）按通用 base_url 形态
        _ => json!({
            "base_url": base_url,
            "apiKey": "tokentap",
        }),
    };
    let display = if name.is_empty() {
        format!("网络节点 · {}", &peer_id[..peer_id.len().min(8)])
    } else {
        format!("网络节点 · {name}")
    };
    let mut provider =
        Provider::with_id(remote_provider_id(peer_id), display, settings_config, None);
    provider.category = Some("share".to_string());
    provider
}

/// 构造指向远端具体 Provider 的合成 Provider。
pub fn synthetic_remote_provider_for_provider(
    app_type: &str,
    peer_id: &str,
    peer_name: &str,
    provider: &ShareProviderInfo,
    bridge_port: u16,
) -> Provider {
    let base_url = format!(
        "http://127.0.0.1:{bridge_port}/peer/{peer_id}/provider/{}",
        provider.provider_id
    );
    let settings_config = match app_type {
        "claude" => json!({
            "env": {
                "ANTHROPIC_BASE_URL": base_url,
                "ANTHROPIC_AUTH_TOKEN": "tokentap",
            }
        }),
        "codex" => {
            // Codex 启动时会直接读取 model/model_provider。共享路由不能沿用
            // 当前本地 Provider 的这两个字段，否则选择 Kimi/智谱等远端
            // Provider 后客户端仍会携带旧模型启动并立即报错。
            let default_model = provider
                .default_model
                .as_deref()
                .or_else(|| provider.models.first().map(String::as_str));
            let mut document = DocumentMut::new();
            document["model_provider"] = value("tokentap_shared");
            if let Some(model) = default_model {
                document["model"] = value(model);
            }
            let mut provider_table = Table::new();
            provider_table["name"] = value(provider.name.clone());
            provider_table["base_url"] = value(base_url.clone());
            provider_table["wire_api"] = value("responses");
            let mut providers_table = Table::new();
            providers_table.insert("tokentap_shared", Item::Table(provider_table));
            document["model_providers"] = Item::Table(providers_table);

            json!({
                "auth": { "OPENAI_API_KEY": "tokentap" },
                "config": document.to_string(),
                "base_url": base_url,
                "modelCatalog": {
                    "models": provider.models.iter().map(|model| json!({ "model": model })).collect::<Vec<_>>()
                },
            })
        }
        "gemini" => json!({
            "env": {
                "GOOGLE_GEMINI_BASE_URL": base_url,
                "GEMINI_API_KEY": "tokentap",
            }
        }),
        _ => json!({
            "base_url": base_url,
            "apiKey": "tokentap",
        }),
    };
    let display = if peer_name.is_empty() {
        format!("网络 · {}", provider.name)
    } else {
        format!("网络 · {} · {}", peer_name, provider.name)
    };
    let mut result = Provider::with_id(
        remote_provider_id_for_provider(peer_id, &provider.provider_id),
        display,
        settings_config,
        None,
    );
    result.category = Some("share".to_string());
    result
}

/// 消费侧路由钩子：按 Agent 的 target 选择本地或共享 Provider。
pub struct ConsumerRouteHook {
    table: Arc<RwLock<RemoteRouteTable>>,
}

impl ConsumerRouteHook {
    pub fn new(table: Arc<RwLock<RemoteRouteTable>>) -> Self {
        Self { table }
    }
}

impl RouteHook for ConsumerRouteHook {
    fn post_select(&self, app_type: &str, selected: Vec<Provider>) -> Vec<Provider> {
        let (bridge_port, remotes, selected_targets) = {
            let snap = match self.table.read() {
                Ok(s) => s,
                Err(_) => return selected,
            };
            (
                snap.bridge_port,
                snap.remotes.clone(),
                snap.selected_targets.get(app_type).cloned(),
            )
        };

        let Some(selection) = selected_targets.filter(|targets| !targets.is_empty()) else {
            return selected;
        };
        // 已选择共享 Provider 时，节点离线必须明确失败，不能静默回落到
        // 本地 Provider，否则界面展示与实际计费/路由对象会不一致。
        if remotes.is_empty() {
            return Vec::new();
        }

        let remote_providers: Vec<Provider> = remotes
            .iter()
            .flat_map(|remote| {
                let provider_routes: Vec<Provider> = remote
                    .providers
                    .iter()
                    .filter(|provider| provider.app == app_type)
                    .filter(|provider| {
                        selection
                            .contains(&remote_target_id(&remote.peer_id, &provider.provider_id))
                    })
                    .map(|provider| {
                        synthetic_remote_provider_for_provider(
                            app_type,
                            &remote.peer_id,
                            &remote.name,
                            provider,
                            bridge_port,
                        )
                    })
                    .collect();
                if provider_routes.is_empty()
                    && remote.providers.is_empty()
                    && remote.supports(app_type)
                {
                    vec![synthetic_remote_provider(
                        app_type,
                        &remote.peer_id,
                        &remote.name,
                        bridge_port,
                    )]
                } else {
                    provider_routes
                }
            })
            .collect();

        remote_providers
    }
}

/// 出借侧路由钩子：白名单过滤 + 按 peer 归因克隆
///
/// 注意：忽略本地 select_providers 的结果（本地 current/故障转移队列与
/// 出借语义无关），改为从 DB 全量读取该应用的供应商后按白名单过滤。
pub struct LenderRouteHook {
    db: Arc<crate::database::Database>,
    peer_id: String,
    whitelist: Arc<RwLock<HashSet<String>>>,
}

impl LenderRouteHook {
    pub fn new(
        db: Arc<crate::database::Database>,
        peer_id: String,
        whitelist: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        Self {
            db,
            peer_id,
            whitelist,
        }
    }
}

impl RouteHook for LenderRouteHook {
    fn post_select(&self, app_type: &str, _selected: Vec<Provider>) -> Vec<Provider> {
        self.select_shared(app_type, None)
    }

    fn post_select_with_target(
        &self,
        app_type: &str,
        _selected: Vec<Provider>,
        target: Option<&str>,
    ) -> Vec<Provider> {
        self.select_shared(app_type, target)
    }
}

impl LenderRouteHook {
    fn select_shared(&self, app_type: &str, target: Option<&str>) -> Vec<Provider> {
        let whitelist = match self.whitelist.read() {
            Ok(w) => w.clone(),
            Err(_) => return Vec::new(),
        };
        let all = match self.db.get_all_providers(app_type) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("[Share] 出借侧读取供应商失败: {e}");
                return Vec::new();
            }
        };

        all.into_values()
            // 只出借白名单内的供应商
            .filter(|p| whitelist.contains(&format!("{app_type}:{}", p.id)))
            // 请求指定了共享 Provider 时，只允许该 Provider 参与路由。
            .filter(|p| target.map_or(true, |target_id| target_id == p.id))
            // 官方/托管类供应商（OAuth 绑定本机）不可外借
            .filter(|p| p.category.as_deref() != Some("official"))
            .map(|mut p| {
                p.id = lend_provider_id(&self.peer_id, &p.id);
                p
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table_with(preference: RoutePreference, remotes: Vec<RemotePeerRoute>) -> RemoteRouteTable {
        RemoteRouteTable {
            preference,
            bridge_port: 15723,
            remotes,
            selected_targets: HashMap::new(),
        }
    }

    fn claude_remote(peer: &str) -> RemotePeerRoute {
        RemotePeerRoute {
            peer_id: peer.to_string(),
            name: String::new(),
            shared_apps: vec!["claude".to_string()],
            providers: Vec::new(),
        }
    }

    #[test]
    fn consumer_hook_routes_each_app_by_selected_target() {
        let local = Provider::with_id("local".to_string(), "本地".to_string(), json!({}), None);
        let table = Arc::new(RwLock::new(table_with(
            RoutePreference::NetworkFirst,
            vec![claude_remote("peer-a")],
        )));
        let hook = ConsumerRouteHook::new(table.clone());
        // 旧版全局偏好不再控制实际路由；未选择共享 target 时继续走本地。
        let out = hook.post_select("claude", vec![local.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "local");

        table.write().unwrap().selected_targets.insert(
            "claude".to_string(),
            HashSet::from([remote_target_id("peer-a", "legacy")]),
        );
        let out = hook.post_select("claude", vec![local.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "share:peer-a");

        // Claude 的选择不会连带改变 Codex。
        let out = hook.post_select("codex", vec![local.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "local");
    }

    #[test]
    fn selected_shared_target_does_not_fallback_to_local_when_peer_is_offline() {
        let local = Provider::with_id("local".to_string(), "本地".to_string(), json!({}), None);
        let mut table = table_with(RoutePreference::LocalOnly, Vec::new());
        table.selected_targets.insert(
            "codex".to_string(),
            HashSet::from([remote_target_id("peer-a", "kimi")]),
        );
        let hook = ConsumerRouteHook::new(Arc::new(RwLock::new(table)));
        assert!(hook.post_select("codex", vec![local]).is_empty());
    }

    #[test]
    fn synthetic_provider_shapes() {
        let p = synthetic_remote_provider("claude", "peer-a", "小明的电脑", 15723);
        assert_eq!(
            p.settings_config["env"]["ANTHROPIC_BASE_URL"]
                .as_str()
                .unwrap(),
            "http://127.0.0.1:15723/peer/peer-a"
        );
        let p = synthetic_remote_provider("codex", "peer-a", "", 15723);
        assert_eq!(
            p.settings_config["base_url"].as_str().unwrap(),
            "http://127.0.0.1:15723/peer/peer-a"
        );
        assert!(p.name.contains("peer-a"));
    }

    #[test]
    fn consumer_hook_filters_selected_provider_targets() {
        let remote = RemotePeerRoute {
            peer_id: "peer-a".to_string(),
            name: "节点 A".to_string(),
            shared_apps: vec!["codex".to_string()],
            providers: vec![
                ShareProviderInfo {
                    app: "codex".to_string(),
                    provider_id: "zhipu".to_string(),
                    name: "智谱".to_string(),
                    models: vec!["glm-4".to_string()],
                    default_model: Some("glm-4".to_string()),
                },
                ShareProviderInfo {
                    app: "codex".to_string(),
                    provider_id: "kimi".to_string(),
                    name: "Kimi".to_string(),
                    models: vec!["moonshot-v1".to_string()],
                    default_model: Some("moonshot-v1".to_string()),
                },
            ],
        };
        let table = Arc::new(RwLock::new(table_with(
            RoutePreference::NetworkOnly,
            vec![remote],
        )));
        table.write().unwrap().selected_targets.insert(
            "codex".to_string(),
            HashSet::from([remote_target_id("peer-a", "kimi")]),
        );
        let hook = ConsumerRouteHook::new(table);
        let out = hook.post_select("codex", Vec::new());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "share:peer-a:kimi");
        assert!(out[0].name.contains("Kimi"));
        let config = out[0].settings_config["config"].as_str().unwrap();
        assert!(config.contains("model = \"moonshot-v1\""));
        assert!(config.contains("model_provider = \"tokentap_shared\""));
        assert!(config.contains("wire_api = \"responses\""));
    }

    #[test]
    fn lender_hook_filters_whitelist_and_retags() {
        let db = Arc::new(crate::database::Database::memory().unwrap());
        let mut official =
            Provider::with_id("off".to_string(), "官方".to_string(), json!({}), None);
        official.category = Some("official".to_string());
        let third = Provider::with_id("third".to_string(), "三方".to_string(), json!({}), None);
        let other = Provider::with_id("other".to_string(), "未白名单".to_string(), json!({}), None);
        db.save_provider("claude", &official).unwrap();
        db.save_provider("claude", &third).unwrap();
        db.save_provider("claude", &other).unwrap();

        let whitelist = Arc::new(RwLock::new(HashSet::from([
            "claude:third".to_string(),
            "claude:off".to_string(),
        ])));
        let hook = LenderRouteHook::new(db, "peer-z".to_string(), whitelist);
        let out = hook.post_select("claude", vec![]);
        // official 被排除、other 不在白名单：只剩 third（重标记为 sharelend:peer-z:third）
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "sharelend:peer-z:third");
    }
}
