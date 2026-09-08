//! 出借链路端到端测试（不依赖 swarm/relay，直接驱动受限 Router）
//!
//! 覆盖施工文档的关键断言：
//! - 出站身份模型：上游只见出借方本机 key，消费方凭据/组网头不外泄
//! - 用量归因：proxy_request_logs.provider_id = sharelend:<peer>:<id>
//! - 门禁：HMAC 鉴权、白名单、限额、黑名单

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, RwLock};

    use axum::body::Body;
    use http::{Request, StatusCode};
    use serde_json::json;
    use serial_test::serial;
    use tokio::sync::RwLock as AsyncRwLock;
    use tower::ServiceExt;

    use crate::database::Database;
    use crate::provider::Provider;
    use crate::share::auth::{body_sha256_hex, generate_nonce, sign_request};
    use crate::share::config::HEADER_AUTH;
    use crate::share::ingress::{build_lend_router, build_lend_state, AdmissionCtx};
    use crate::share::types::ShareQuotaConfig;

    const PEER: &str = "12D3KooWTestPeerAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    const SHARE_KEY: &str = "dGVzdC1zaGFyZS1rZXktMzItYnl0ZXMtbG9uZw";

    struct TempHome {
        _dir: tempfile::TempDir,
        original_home: Option<String>,
        original_test_home: Option<String>,
    }

    impl TempHome {
        fn new() -> Self {
            let dir = tempfile::TempDir::new().expect("temp home");
            let original_home = std::env::var("HOME").ok();
            let original_test_home = std::env::var("CC_SWITCH_TEST_HOME").ok();
            std::env::set_var("HOME", dir.path());
            std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
            crate::settings::reload_settings().expect("reload settings");
            Self {
                _dir: dir,
                original_home,
                original_test_home,
            }
        }
    }

    impl Drop for TempHome {
        fn drop(&mut self) {
            match &self.original_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.original_test_home {
                Some(v) => std::env::set_var("CC_SWITCH_TEST_HOME", v),
                None => std::env::remove_var("CC_SWITCH_TEST_HOME"),
            }
        }
    }

    #[derive(Debug)]
    struct CapturedHeaders {
        authorization: String,
        cc_switch_remote_auth: String,
        consumer_credentials: Vec<String>,
    }

    /// 启动模拟上游，返回 (地址, 捕获的请求头)
    async fn start_mock_upstream() -> (String, Arc<Mutex<Vec<CapturedHeaders>>>) {
        let captured: Arc<Mutex<Vec<CapturedHeaders>>> = Arc::new(Mutex::new(Vec::new()));
        let captured2 = captured.clone();
        let app = axum::Router::new().route(
            "/v1/messages",
            axum::routing::post(move |req: http::Request<axum::body::Body>| {
                let captured = captured2.clone();
                async move {
                    let auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let cc_switch_remote_auth = req
                        .headers()
                        .get("x-cc-switch-remote-auth")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let consumer_credentials = [
                        "proxy-authorization",
                        "cookie",
                        "api-key",
                        "x-api-key",
                        "x-goog-api-key",
                        "chatgpt-account-id",
                        "openai-organization",
                        "openai-project",
                    ]
                    .into_iter()
                    .filter_map(|name| {
                        req.headers()
                            .get(name)
                            .and_then(|value| value.to_str().ok())
                            .map(|value| format!("{name}:{value}"))
                    })
                    .collect();
                    captured.lock().unwrap().push(CapturedHeaders {
                        authorization: auth,
                        cc_switch_remote_auth,
                        consumer_credentials,
                    });
                    axum::Json(json!({
                        "id": "msg_mock",
                        "type": "message",
                        "role": "assistant",
                        "model": "claude-mock",
                        "content": [{"type": "text", "text": "mock-ok"}],
                        "stop_reason": "end_turn",
                        "usage": {"input_tokens": 11, "output_tokens": 7}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, captured)
    }

    fn signed_request(
        key: &str,
        path: &str,
        body: &serde_json::Value,
        extra_auth: Option<&str>,
    ) -> Request<Body> {
        let body_bytes = serde_json::to_vec(body).unwrap();
        let hash = body_sha256_hex(&body_bytes);
        let now = chrono::Utc::now().timestamp();
        let nonce = generate_nonce();
        let sig = sign_request(key, now, &nonce, "POST", path, &hash).unwrap();
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .header(HEADER_AUTH, &sig);
        if let Some(a) = extra_auth {
            builder = builder.header("authorization", a);
        }
        builder.body(Body::from(body_bytes)).unwrap()
    }

    struct LendFixture {
        db: Arc<Database>,
        router: axum::Router,
        captured: Arc<Mutex<Vec<CapturedHeaders>>>,
        _oauth_dir: tempfile::TempDir,
    }

    async fn lend_fixture(whitelist: HashSet<String>, quota: ShareQuotaConfig) -> LendFixture {
        // The desktop runtime installs the ring provider during app setup. These
        // router-level tests bypass that setup, so mirror it before reqwest
        // constructs a rustls client in the forwarding path.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let db = Arc::new(Database::memory().unwrap());
        let (mock_addr, captured) = start_mock_upstream().await;
        let provider = Provider::with_id(
            "third".to_string(),
            "三方Claude".to_string(),
            json!({
                "env": {
                    "ANTHROPIC_BASE_URL": format!("http://{mock_addr}"),
                    "ANTHROPIC_AUTH_TOKEN": "lender-real-secret-key",
                }
            }),
            None,
        );
        db.save_provider("claude", &provider).unwrap();

        let whitelist = Arc::new(RwLock::new(whitelist));
        let oauth_dir = tempfile::TempDir::new().unwrap();
        let oauth_manager = Arc::new(
            crate::proxy::providers::codex_oauth_auth::CodexOAuthManager::new(
                oauth_dir.path().to_path_buf(),
            ),
        );
        let state = build_lend_state(db.clone(), PEER, whitelist.clone(), None).await;
        let ctx = AdmissionCtx::new(
            db.clone(),
            PEER.to_string(),
            Arc::new(AsyncRwLock::new(Some(SHARE_KEY.to_string()))),
            Arc::new(AsyncRwLock::new(quota)),
            whitelist,
            "出借方测试机".to_string(),
            oauth_manager,
        );
        LendFixture {
            db,
            router: build_lend_router(state, ctx),
            captured,
            _oauth_dir: oauth_dir,
        }
    }

    fn whitelist_third() -> HashSet<String> {
        HashSet::from(["claude:third".to_string()])
    }

    fn unlimited_quota() -> ShareQuotaConfig {
        ShareQuotaConfig {
            scope: "daily".to_string(),
            max_tokens: 0,
            per_peer: true,
        }
    }

    fn chat_body() -> serde_json::Value {
        json!({
            "model": "claude-mock",
            "max_tokens": 32,
            "messages": [{"role": "user", "content": "hi"}]
        })
    }

    #[tokio::test]
    #[serial]
    async fn lend_full_path_attribution_and_egress_identity() {
        let _home = TempHome::new();
        let fx = lend_fixture(whitelist_third(), unlimited_quota()).await;

        // 消费方请求（自带一个无关的 authorization，必须被忽略）
        let mut req = signed_request(
            SHARE_KEY,
            "/v1/messages",
            &chat_body(),
            Some("Bearer consumer-junk-token"),
        );
        for (name, value) in [
            ("proxy-authorization", "Basic consumer-proxy-secret"),
            ("cookie", "consumer_session=secret"),
            ("api-key", "consumer-api-key"),
            ("x-api-key", "consumer-x-api-key"),
            ("x-goog-api-key", "consumer-google-key"),
            ("chatgpt-account-id", "consumer-account"),
            ("openai-organization", "consumer-org"),
            ("openai-project", "consumer-project"),
        ] {
            req.headers_mut().insert(
                http::HeaderName::from_static(name),
                http::HeaderValue::from_static(value),
            );
        }
        let resp = fx.router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["content"][0]["text"], "mock-ok");

        // 出站身份模型：上游收到的是出借方 key，且组网头不外泄
        let captured = fx.captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].authorization, "Bearer lender-real-secret-key");
        assert_eq!(captured[0].cc_switch_remote_auth, "");
        assert!(
            captured[0].consumer_credentials.is_empty(),
            "消费方认证/账号头不应到达上游: {:?}",
            captured[0].consumer_credentials
        );

        // 用量归因：proxy_request_logs 按 sharelend:<peer>:third 落账
        let used = fx.db.share_peer_tokens_used(PEER, 0).unwrap();
        assert_eq!(used, 18, "input 11 + output 7 应归因到该 peer");
    }

    #[tokio::test]
    #[serial]
    async fn lend_rejects_bad_signature() {
        let _home = TempHome::new();
        let fx = lend_fixture(whitelist_third(), unlimited_quota()).await;
        let req = signed_request(
            "b3RoZXIta2V5LXRvdGFsbHktd3JvbmctMzItYnl0ZQ",
            "/v1/messages",
            &chat_body(),
            None,
        );
        let resp = fx.router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(fx.captured.lock().unwrap().is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn lend_rejects_non_whitelisted_app() {
        let _home = TempHome::new();
        let fx = lend_fixture(whitelist_third(), unlimited_quota()).await;
        // claude 在白名单，但请求 gemini 路径 → 403
        let body = json!({"contents": []});
        let req = signed_request(SHARE_KEY, "/v1beta/models/g:generateContent", &body, None);
        let resp = fx.router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    #[serial]
    async fn lend_quota_exceeded_returns_429() {
        let _home = TempHome::new();
        let fx = lend_fixture(
            whitelist_third(),
            ShareQuotaConfig {
                scope: "daily".to_string(),
                max_tokens: 100,
                per_peer: true,
            },
        )
        .await;
        // 预置已用量 100（本周期内）
        {
            let conn = fx.db.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, input_tokens,
                    output_tokens, latency_ms, status_code, created_at
                 ) VALUES ('pre', ?1, 'claude', 'm', 60, 40, 1, 200, ?2)",
                rusqlite::params![
                    format!("sharelend:{PEER}:third"),
                    chrono::Utc::now().timestamp()
                ],
            )
            .unwrap();
        }
        let req = signed_request(SHARE_KEY, "/v1/messages", &chat_body(), None);
        let resp = fx.router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    #[serial]
    async fn lend_blocked_peer_returns_403() {
        let _home = TempHome::new();
        let fx = lend_fixture(whitelist_third(), unlimited_quota()).await;
        fx.db.block_share_peer(PEER, Some("滥用")).unwrap();
        let req = signed_request(SHARE_KEY, "/v1/messages", &chat_body(), None);
        let resp = fx.router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
