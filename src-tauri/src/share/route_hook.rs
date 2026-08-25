//! 组网路由钩子：消费侧远端注入 / 出借侧白名单过滤
//!
//! 消费侧：把在线的远端节点合成为普通 Provider（base_url 指向本机桥接
//! `http://127.0.0.1:{bridge}/peer/{peer_id}`），按路由偏好与本地路由合并，
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

use super::types::RoutePreference;

/// 消费侧远端路由快照（ShareManager 异步维护，钩子同步读取）
#[derive(Debug, Clone, Default)]
pub struct RemoteRouteTable {
    pub preference: RoutePreference,
    pub bridge_port: u16,
    /// 远端节点路由项
    pub remotes: Vec<RemotePeerRoute>,
}

/// 单个远端节点的路由信息
#[derive(Debug, Clone)]
pub struct RemotePeerRoute {
    pub peer_id: String,
    pub name: String,
    /// 该节点共享的应用类型
    pub shared_apps: Vec<String>,
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

/// 消费侧路由钩子：把远端节点按偏好合并进路由列表
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
        let (preference, bridge_port, remotes) = {
            let snap = match self.table.read() {
                Ok(s) => s,
                Err(_) => return selected,
            };
            (snap.preference, snap.bridge_port, snap.remotes.clone())
        };

        if remotes.is_empty() || preference == RoutePreference::LocalOnly {
            return selected;
        }

        let remote_providers: Vec<Provider> = remotes
            .iter()
            .filter(|r| r.supports(app_type))
            .map(|r| synthetic_remote_provider(app_type, &r.peer_id, &r.name, bridge_port))
            .collect();

        match preference {
            RoutePreference::LocalOnly => selected,
            RoutePreference::LocalFirst => {
                let mut out = selected;
                out.extend(remote_providers);
                out
            }
            RoutePreference::NetworkFirst => {
                let mut out = remote_providers;
                out.extend(selected);
                out
            }
            RoutePreference::NetworkOnly => remote_providers,
        }
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
        }
    }

    fn claude_remote(peer: &str) -> RemotePeerRoute {
        RemotePeerRoute {
            peer_id: peer.to_string(),
            name: String::new(),
            shared_apps: vec!["claude".to_string()],
        }
    }

    #[test]
    fn consumer_hook_respects_preference() {
        let local = Provider::with_id("local".to_string(), "本地".to_string(), json!({}), None);
        let table = Arc::new(RwLock::new(table_with(
            RoutePreference::NetworkFirst,
            vec![claude_remote("peer-a")],
        )));
        let hook = ConsumerRouteHook::new(table.clone());
        let out = hook.post_select("claude", vec![local.clone()]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "share:peer-a");
        assert_eq!(out[1].id, "local");

        table.write().unwrap().preference = RoutePreference::LocalFirst;
        let out = hook.post_select("claude", vec![local.clone()]);
        assert_eq!(out[0].id, "local");

        table.write().unwrap().preference = RoutePreference::NetworkOnly;
        let out = hook.post_select("claude", vec![local.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "share:peer-a");

        // 不共享 codex 的节点不应出现在 codex 路由中
        let out = hook.post_select("codex", vec![local.clone()]);
        assert!(out.is_empty());

        table.write().unwrap().preference = RoutePreference::LocalOnly;
        let out = hook.post_select("claude", vec![local.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "local");
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
