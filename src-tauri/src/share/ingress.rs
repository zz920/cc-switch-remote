//! 出借侧数据面：P2P stream 上的 HTTP 服务
//!
//! 每个入站数据面 stream：
//! 1. 门禁（黑名单 → HMAC 校验+防重放 → 出口净化 → 白名单/限额准入）
//! 2. 净化后的请求交给复用现有 handlers 的受限 axum Router
//! 3. 受限 Router 使用按 peer 归因的合成供应商（sharelend:<peer>:<id>），
//!    由出借方本机注入自己的 key、从本机 IP 向上游重新发起请求
//!
//! 响应（含 SSE 流式）沿同一 stream 原样回传消费方。

use axum::{extract::State, middleware, response::Response, Router};
use http::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::database::Database;
use crate::provider::Provider;
use crate::proxy::failover_switch::FailoverSwitchManager;
use crate::proxy::provider_router::ProviderRouter;
use crate::proxy::providers::codex_chat_history::CodexChatHistoryStore;
use crate::proxy::providers::gemini_shadow::GeminiShadowStore;
use crate::proxy::server::ProxyState;
use crate::proxy::types::{ProxyConfig, ProxyStatus};

use super::auth::{body_sha256_hex, verify_request, NonceCache};
use super::bridge::error_response;
use super::config::{
    AUTH_WINDOW_SECS, HEADER_AUTH, HEADER_PREFIX, HEADER_ROUTE_PROVIDER, META_PATH,
    NONCE_CACHE_CAPACITY, PROVIDER_CHECK_PATH,
};
use super::official::{
    is_structurally_shareable, managed_codex_official_account_id, SharedRequestContext,
};
use super::quota::check_lend_quota;
use super::route_hook::LenderRouteHook;
use super::types::{ShareProviderInfo, ShareQuotaConfig};

/// 出借准入上下文（ShareManager 共享持有，peer 维度）
#[derive(Clone)]
pub struct AdmissionCtx {
    pub db: Arc<Database>,
    pub peer_id: String,
    /// 当前 share key（热更新：重生成后所有在途校验立即用新 key）
    pub share_key: Arc<AsyncRwLock<Option<String>>>,
    pub nonce_cache: Arc<std::sync::Mutex<NonceCache>>,
    pub quota: Arc<AsyncRwLock<ShareQuotaConfig>>,
    /// 白名单："app:provider_id" 集合
    pub whitelist: Arc<RwLock<HashSet<String>>>,
    pub node_name: String,
    pub codex_oauth_manager: Arc<crate::proxy::providers::codex_oauth_auth::CodexOAuthManager>,
}

impl AdmissionCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: Arc<Database>,
        peer_id: String,
        share_key: Arc<AsyncRwLock<Option<String>>>,
        quota: Arc<AsyncRwLock<ShareQuotaConfig>>,
        whitelist: Arc<RwLock<HashSet<String>>>,
        node_name: String,
        codex_oauth_manager: Arc<crate::proxy::providers::codex_oauth_auth::CodexOAuthManager>,
    ) -> Self {
        Self {
            db,
            peer_id,
            share_key,
            nonce_cache: Arc::new(std::sync::Mutex::new(NonceCache::new(NONCE_CACHE_CAPACITY))),
            quota,
            whitelist,
            node_name,
            codex_oauth_manager,
        }
    }
}

/// 构建出借侧受限 ProxyState（按 peer 缓存复用）
pub async fn build_lend_state(
    db: Arc<Database>,
    peer_id: &str,
    whitelist: Arc<RwLock<HashSet<String>>>,
    app_handle: Option<tauri::AppHandle>,
) -> ProxyState {
    let router = Arc::new(ProviderRouter::new(db.clone()));
    router
        .set_route_hook(Some(Arc::new(LenderRouteHook::new(
            db.clone(),
            peer_id.to_string(),
            whitelist,
        ))))
        .await;

    ProxyState {
        db: db.clone(),
        config: Arc::new(AsyncRwLock::new(ProxyConfig::default())),
        status: Arc::new(AsyncRwLock::new(ProxyStatus::default())),
        start_time: Arc::new(AsyncRwLock::new(None)),
        current_providers: Arc::new(AsyncRwLock::new(std::collections::HashMap::new())),
        provider_router: router,
        gemini_shadow: Arc::new(GeminiShadowStore::default()),
        codex_chat_history: Arc::new(CodexChatHistoryStore::default()),
        app_handle,
        failover_manager: Arc::new(FailoverSwitchManager::new(db)),
    }
}

/// 构建出借侧受限 Router（受限 ProxyState + 门禁中间件）
///
/// 抽出为独立函数：`serve_lend_stream` 与集成测试共用。
pub fn build_lend_router(state: ProxyState, ctx: AdmissionCtx) -> Router {
    Router::new()
        .fallback_service(crate::proxy::server::build_proxy_router(state))
        .layer(middleware::from_fn_with_state(Arc::new(ctx), admission_mw))
}

/// 出借侧服务一个入站数据面 stream
pub async fn serve_lend_stream(stream: libp2p::Stream, state: ProxyState, ctx: AdmissionCtx) {
    let router = build_lend_router(state, ctx);

    let io = TokioIo::new(stream.compat());
    let service = hyper::service::service_fn(move |req: http::Request<hyper::body::Incoming>| {
        let mut router = router.clone();
        async move {
            let (parts, body) = req.into_parts();
            let req = http::Request::from_parts(parts, axum::body::Body::new(body));
            <Router as tower::Service<http::Request<axum::body::Body>>>::call(&mut router, req)
                .await
        }
    });

    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .serve_connection(io, service)
        .await
    {
        log::debug!("[Share] 出借侧连接结束: {e}");
    }
}

/// 从路径推断应用类型（与代理路由的路径前缀对齐）
fn infer_app_type(path: &str) -> Option<&'static str> {
    if path.starts_with("/v1/messages") || path.starts_with("/claude/") {
        Some("claude")
    } else if path.starts_with("/claude-desktop/") {
        Some("claude_desktop")
    } else if path.contains("chat/completions")
        || path.starts_with("/responses")
        || path.starts_with("/codex/")
        || path.starts_with("/models")
        || path.contains("alpha/search")
    {
        Some("codex")
    } else if path.starts_with("/v1beta/") || path.starts_with("/gemini/") {
        Some("gemini")
    } else if path.starts_with("/grokbuild/") {
        Some("grokbuild")
    } else {
        None
    }
}

/// 消费方携带的认证与上游账号选择头。
///
/// 共享请求只能使用出借方本机 Provider 的凭据。这些头即使会在普通
/// Provider 路径中被 forwarder 覆盖，也必须在门禁层先删除，避免新增的
/// passthrough/Official 路径意外把消费方身份带到真实上游。
fn is_consumer_credential_header(name: &http::HeaderName) -> bool {
    matches!(
        name.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "api-key"
            | "x-api-key"
            | "x-goog-api-key"
            | "chatgpt-account-id"
            | "openai-organization"
            | "openai-project"
    )
}

/// 门禁中间件：黑名单 → HMAC → 净化 → 白名单 → 限额
async fn admission_mw(
    State(ctx): State<Arc<AdmissionCtx>>,
    req: Request<axum::body::Body>,
    next: middleware::Next,
) -> Response {
    // 1. 黑名单
    match ctx.db.is_share_peer_blocked(&ctx.peer_id) {
        Ok(true) => {
            return error_response(StatusCode::FORBIDDEN, "节点已被出借方拉黑");
        }
        Err(e) => {
            log::warn!("[Share] 黑名单检查失败: {e}");
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "内部错误");
        }
        _ => {}
    }

    // 2. HMAC 鉴权
    let auth_header = req
        .headers()
        .get(HEADER_AUTH)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let Some(auth_header) = auth_header else {
        return error_response(StatusCode::UNAUTHORIZED, "缺少组网鉴权头");
    };
    let Some(key) = ctx.share_key.read().await.clone() else {
        return error_response(StatusCode::SERVICE_UNAVAILABLE, "出借方未就绪");
    };

    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, 256 * 1024 * 1024).await {
        Ok(b) => b,
        Err(e) => {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                &format!("读取请求体失败: {e}"),
            )
        }
    };
    let body_hash = body_sha256_hex(&body_bytes);
    let now = chrono::Utc::now().timestamp();
    let fresh = {
        let mut cache = match ctx.nonce_cache.lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        verify_request(
            &key,
            &auth_header,
            method.as_str(),
            &path,
            &body_hash,
            now,
            AUTH_WINDOW_SECS,
            &mut cache,
        )
    };
    if !fresh {
        return error_response(StatusCode::UNAUTHORIZED, "组网鉴权失败（签名/时间戳/重放）");
    }

    // 3. meta 能力查询（不计配额）
    if path == META_PATH {
        return meta_response(&ctx).await;
    }

    // 共享 Provider 的连通性检查只探测出借方配置的 base URL，
    // 不发送模型请求、不计 token；仍要求完整 HMAC 和 Provider 白名单。
    if path == PROVIDER_CHECK_PATH {
        return provider_check_response(&ctx, &parts).await;
    }

    // 4. 白名单准入（按路径推断的应用类型）
    let app_type = infer_app_type(&path);
    let route_provider = match parts.headers.get(HEADER_ROUTE_PROVIDER) {
        Some(value) => match value.to_str().map(str::trim) {
            Ok("") => None,
            Ok(value) => Some(value.to_string()),
            Err(_) => return error_response(StatusCode::BAD_REQUEST, "共享 Provider target 无效"),
        },
        None => None,
    };
    let whitelisted_app = {
        let whitelist = ctx.whitelist.read().map(|w| w.clone()).unwrap_or_default();
        match app_type {
            Some(app) => match route_provider.as_deref() {
                Some(provider_id) => whitelist.contains(&format!("{app}:{provider_id}")),
                None => whitelist
                    .iter()
                    .any(|entry| entry.starts_with(&format!("{app}:"))),
            },
            None => false,
        }
    };
    if !whitelisted_app {
        return error_response(StatusCode::FORBIDDEN, "出借方未共享该应用类型的供应商");
    }

    // 5. 限额准入
    let (scope, max_tokens, per_peer) = {
        let q = ctx.quota.read().await;
        (q.scope.clone(), q.max_tokens, q.per_peer)
    };
    match check_lend_quota(&ctx.db, &ctx.peer_id, &scope, max_tokens, per_peer) {
        Ok(check) if !check.allowed => {
            let mut resp = error_response(StatusCode::TOO_MANY_REQUESTS, "出借方配额已用尽");
            resp.headers_mut().insert(
                http::header::RETRY_AFTER,
                http::HeaderValue::from_static("3600"),
            );
            return resp;
        }
        Err(e) => {
            log::warn!("[Share] 限额检查失败: {e}");
        }
        _ => {}
    }

    // 6. 出口净化：剥离组网头和消费方凭据后重建请求。
    // Provider target 仍需保留给出借方路由钩子，并由 forwarder 在真实出站前删除。
    let mut builder = http::Request::builder().method(method).uri(parts.uri);
    for (name, value) in parts.headers.iter() {
        let is_internal_header = name.as_str().starts_with(HEADER_PREFIX);
        let is_route_target = name.as_str() == HEADER_ROUTE_PROVIDER;
        if is_consumer_credential_header(name) || (is_internal_header && !is_route_target) {
            continue;
        }
        builder = builder.header(name, value);
    }
    // 出借方本地 forwarder 会根据被选中的 Provider 注入本机凭据。
    let sanitized = builder.body(axum::body::Body::from(body_bytes));
    let mut sanitized = match sanitized {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("重建请求失败: {e}")),
    };
    sanitized.extensions_mut().insert(SharedRequestContext {
        peer_id: ctx.peer_id.clone(),
        app_type: app_type.unwrap_or_default().to_string(),
        provider_id: route_provider,
    });

    next.run(sanitized).await
}

async fn provider_check_response(ctx: &AdmissionCtx, parts: &http::request::Parts) -> Response {
    let Some(provider_id) = parts
        .headers
        .get(HEADER_ROUTE_PROVIDER)
        .and_then(|value| value.to_str().ok())
    else {
        return error_response(StatusCode::BAD_REQUEST, "缺少共享 Provider target");
    };
    let Some(app_type) = parts
        .uri
        .query()
        .and_then(|query| query.split('&').find_map(|item| item.strip_prefix("app=")))
    else {
        return error_response(StatusCode::BAD_REQUEST, "缺少共享 Provider 应用类型");
    };
    let allowed = ctx
        .whitelist
        .read()
        .map(|whitelist| whitelist.contains(&format!("{app_type}:{provider_id}")))
        .unwrap_or(false);
    if !allowed {
        return error_response(StatusCode::FORBIDDEN, "共享 Provider 不在出借白名单中");
    }
    let Ok(Some(provider)) = ctx.db.get_provider_by_id(provider_id, app_type) else {
        return error_response(StatusCode::NOT_FOUND, "共享 Provider 不存在");
    };
    if provider.category.as_deref() == Some("official") {
        let Some(account_id) = managed_codex_official_account_id(app_type, &provider) else {
            return error_response(StatusCode::FORBIDDEN, "该 Official Provider 不符合共享条件");
        };
        if !ctx.codex_oauth_manager.has_account(&account_id).await {
            return error_response(
                StatusCode::UNAUTHORIZED,
                "Official Provider 绑定的本机 OAuth 账号不可用",
            );
        }
        if let Err(error) = ctx
            .codex_oauth_manager
            .get_valid_token_for_account(&account_id)
            .await
        {
            log::warn!(
                "[Share] Official Provider OAuth 自检失败（provider={provider_id}）: {error}"
            );
            return error_response(
                StatusCode::UNAUTHORIZED,
                "Official Provider OAuth 凭据不可用，请联系出借方重新登录",
            );
        }
        let body = serde_json::json!({
            "status": "operational",
            "success": true,
            "message": "本机托管 OAuth 凭据可用",
            "responseTimeMs": null,
            "httpStatus": null,
            "modelUsed": "",
            "testedAt": chrono::Utc::now().timestamp(),
            "retryCount": 0,
            "errorCategory": null,
        });
        let mut response = Response::new(axum::body::Body::from(body.to_string()));
        *response.status_mut() = StatusCode::OK;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        return response;
    }
    let Ok(app) = crate::app_config::AppType::from_str(app_type) else {
        return error_response(StatusCode::BAD_REQUEST, "不支持的共享 Provider 应用类型");
    };
    let Ok(config) = ctx.db.get_stream_check_config() else {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, "读取连通性检查配置失败");
    };
    let result = match crate::services::stream_check::StreamCheckService::check_with_retry(
        &app, &provider, &config, None,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => crate::services::stream_check::StreamCheckResult {
            status: crate::services::stream_check::HealthStatus::Failed,
            success: false,
            message: error.to_string(),
            response_time_ms: None,
            http_status: None,
            model_used: String::new(),
            tested_at: chrono::Utc::now().timestamp(),
            retry_count: 0,
            error_category: None,
        },
    };
    let body = serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string());
    let mut response = Response::new(axum::body::Body::from(body));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    response
}

/// 能力通告应答
async fn meta_response(ctx: &AdmissionCtx) -> Response {
    let whitelist = ctx.whitelist.read().map(|w| w.clone()).unwrap_or_default();
    let providers = shared_provider_infos(&ctx.db, &whitelist, &ctx.codex_oauth_manager).await;
    let shared_apps: Vec<String> = {
        let mut apps: Vec<String> = providers
            .iter()
            .map(|provider| provider.app.clone())
            .collect();
        apps.sort();
        apps.dedup();
        apps
    };
    let (scope, max_tokens, per_peer) = {
        let q = ctx.quota.read().await;
        (q.scope.clone(), q.max_tokens, q.per_peer)
    };
    let body = serde_json::json!({
        "name": ctx.node_name,
        "peerId": ctx.peer_id,
        "sharedApps": shared_apps,
        "providers": providers,
        "quota": {
            "scope": scope,
            "maxTokens": max_tokens,
            "perPeer": per_peer,
        },
    });
    let mut resp = Response::new(axum::body::Body::from(body.to_string()));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    resp
}

/// 生成可安全公开给网络成员的 Provider 摘要。
/// 这里只返回应用、Provider 名称和模型，不暴露 settings/auth/API key。
async fn shared_provider_infos(
    db: &Database,
    whitelist: &HashSet<String>,
    codex_oauth_manager: &crate::proxy::providers::codex_oauth_auth::CodexOAuthManager,
) -> Vec<ShareProviderInfo> {
    let mut out = Vec::new();
    for entry in whitelist {
        let Some((app, provider_id)) = entry.split_once(':') else {
            continue;
        };
        let Ok(Some(provider)) = db.get_provider_by_id(provider_id, app) else {
            continue;
        };
        if !is_structurally_shareable(app, &provider) {
            continue;
        }
        let managed_account = managed_codex_official_account_id(app, &provider);
        if let Some(account_id) = managed_account.as_deref() {
            if !codex_oauth_manager.has_account(account_id).await {
                continue;
            }
        }
        // Official 托管 OAuth：模型由 ChatGPT 账号与消费方 Codex CLI 动态决定，
        // 通告任何具体模型名都会过时（新模型上线后误导消费方把旧默认写死进
        // config.toml）。因此 models/default_model 留空，仅带 auth_mode 标记；
        // 消费方据此激活时不写 model，CLI 使用自身默认模型并随其升级演进。
        let (models, default_model) = if managed_account.is_some() {
            (Vec::new(), None)
        } else {
            (
                provider_models(&provider),
                provider_default_model(&provider),
            )
        };
        out.push(ShareProviderInfo {
            app: app.to_string(),
            provider_id: provider.id.clone(),
            name: provider.name.clone(),
            models,
            default_model,
            auth_mode: managed_account.map(|_| "managed_oauth".to_string()),
        });
    }
    out.sort_by(|a, b| a.app.cmp(&b.app).then(a.name.cmp(&b.name)));
    out
}

/// 只通告模型标识，不通告 base URL、认证信息或其余 Provider 配置。
fn provider_default_model(provider: &Provider) -> Option<String> {
    let config = provider
        .settings_config
        .get("config")
        .and_then(|value| value.as_str())?;
    let document = config.parse::<toml_edit::DocumentMut>().ok()?;
    document
        .get("model")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn provider_models(provider: &Provider) -> Vec<String> {
    let mut models = provider
        .settings_config
        .get("modelCatalog")
        .and_then(|catalog| catalog.get("models"))
        .and_then(|items| items.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    item.get("model")
                        .or_else(|| item.get("id"))
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(default_model) = provider_default_model(provider) {
        models.push(default_model);
    }
    models.sort();
    models.dedup();
    models.truncate(12);
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn infer_app_type_paths() {
        assert_eq!(infer_app_type("/v1/messages"), Some("claude"));
        assert_eq!(infer_app_type("/claude/v1/messages"), Some("claude"));
        assert_eq!(infer_app_type("/v1/chat/completions"), Some("codex"));
        assert_eq!(infer_app_type("/codex/v1/responses"), Some("codex"));
        assert_eq!(
            infer_app_type("/v1beta/models/g:generateContent"),
            Some("gemini")
        );
        assert_eq!(infer_app_type("/unknown"), None);
    }

    #[test]
    fn consumer_credential_headers_are_classified_for_removal() {
        for name in [
            "authorization",
            "proxy-authorization",
            "cookie",
            "api-key",
            "x-api-key",
            "x-goog-api-key",
            "chatgpt-account-id",
            "openai-organization",
            "openai-project",
        ] {
            let name = http::HeaderName::from_static(name);
            assert!(
                is_consumer_credential_header(&name),
                "{name} should be removed from shared ingress"
            );
        }

        assert!(!is_consumer_credential_header(&http::header::CONTENT_TYPE));
        assert!(!is_consumer_credential_header(
            &http::HeaderName::from_static(HEADER_ROUTE_PROVIDER)
        ));
    }

    #[test]
    fn provider_capability_uses_top_level_codex_model_as_default() {
        let provider = Provider::with_id(
            "kimi".to_string(),
            "Kimi".to_string(),
            json!({
                "config": "model_provider = \"kimi\"\nmodel = \"kimi-k2.5\"\n\n[model_providers.kimi]\nbase_url = \"https://example.invalid/v1\"\n",
                "modelCatalog": {
                    "models": [
                        { "model": "kimi-k2" },
                        { "model": "kimi-k2.5" }
                    ]
                }
            }),
            None,
        );

        assert_eq!(
            provider_default_model(&provider).as_deref(),
            Some("kimi-k2.5")
        );
        assert_eq!(
            provider_models(&provider),
            vec!["kimi-k2".to_string(), "kimi-k2.5".to_string()]
        );
    }
}
