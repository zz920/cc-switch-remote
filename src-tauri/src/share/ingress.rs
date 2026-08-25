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
use std::sync::{Arc, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::database::Database;
use crate::proxy::failover_switch::FailoverSwitchManager;
use crate::proxy::provider_router::ProviderRouter;
use crate::proxy::providers::codex_chat_history::CodexChatHistoryStore;
use crate::proxy::providers::gemini_shadow::GeminiShadowStore;
use crate::proxy::server::ProxyState;
use crate::proxy::types::{ProxyConfig, ProxyStatus};

use super::auth::{body_sha256_hex, verify_request, NonceCache};
use super::bridge::error_response;
use super::config::{
    AUTH_WINDOW_SECS, HEADER_AUTH, HEADER_PREFIX, META_PATH, NONCE_CACHE_CAPACITY,
};
use super::quota::check_lend_quota;
use super::route_hook::LenderRouteHook;
use super::types::ShareQuotaConfig;

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
    ) -> Self {
        Self {
            db,
            peer_id,
            share_key,
            nonce_cache: Arc::new(std::sync::Mutex::new(NonceCache::new(NONCE_CACHE_CAPACITY))),
            quota,
            whitelist,
            node_name,
        }
    }
}

/// 构建出借侧受限 ProxyState（按 peer 缓存复用）
pub async fn build_lend_state(
    db: Arc<Database>,
    peer_id: &str,
    whitelist: Arc<RwLock<HashSet<String>>>,
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
        app_handle: None,
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

    // 4. 白名单准入（按路径推断的应用类型）
    let app_type = infer_app_type(&path);
    let whitelisted_app = {
        let whitelist = ctx.whitelist.read().map(|w| w.clone()).unwrap_or_default();
        match app_type {
            Some(app) => whitelist
                .iter()
                .any(|entry| entry.starts_with(&format!("{app}:"))),
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

    // 6. 出口净化：剥离所有组网头部后重建请求
    let mut builder = http::Request::builder().method(method).uri(parts.uri);
    for (name, value) in parts.headers.iter() {
        if !name.as_str().starts_with(HEADER_PREFIX) {
            builder = builder.header(name, value);
        }
    }
    // 消费方凭据一律不带入（出借方本地 forwarder 会注入本机 key）
    let sanitized = builder.body(axum::body::Body::from(body_bytes));
    let sanitized = match sanitized {
        Ok(r) => r,
        Err(e) => return error_response(StatusCode::BAD_REQUEST, &format!("重建请求失败: {e}")),
    };

    next.run(sanitized).await
}

/// 能力通告应答
async fn meta_response(ctx: &AdmissionCtx) -> Response {
    let shared_apps: Vec<String> = {
        let whitelist = ctx.whitelist.read().map(|w| w.clone()).unwrap_or_default();
        let mut apps: Vec<String> = whitelist
            .iter()
            .filter_map(|entry| entry.split(':').next().map(|s| s.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
