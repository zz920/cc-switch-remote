//! 消费侧本地桥接：127.0.0.1:15723 → P2P stream
//!
//! 远端节点在路由表中以普通供应商形态出现（base_url 指向本桥接），
//! 桥接把 HTTP 请求经 P2P stream 转发给出借方，响应原样回流。
//! 对本地 forwarder 完全透明：熔断/故障转移/协议转换零改动复用。

use axum::{extract::State, response::Response, routing::any, Router};
use hyper_util::rt::TokioIo;
use tokio::sync::oneshot;

use super::config::HEADER_ROUTE_PROVIDER;
use super::ShareManager;

/// 桥接服务器句柄
pub struct BridgeHandle {
    port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl BridgeHandle {
    pub fn port(&self) -> u16 {
        self.port
    }
    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), task).await;
        }
    }
}

#[derive(Clone)]
struct BridgeState {
    manager: ShareManager,
}

/// 启动消费侧桥接（仅 loopback）
pub async fn start_bridge(manager: ShareManager, port: u16) -> Result<BridgeHandle, String> {
    let state = BridgeState { manager };
    let app = Router::new()
        .route("/*rest", any(bridge_handler))
        .route("/", any(bridge_handler))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("桥接端口 {port} 绑定失败: {e}"))?;
    let actual_port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, _)) => {
                            let app = app.clone();
                            tokio::spawn(async move {
                                let io = TokioIo::new(stream);
                                let service = hyper::service::service_fn(
                                    move |req: http::Request<hyper::body::Incoming>| {
                                        let mut router = app.clone();
                                        async move {
                                            let (parts, body) = req.into_parts();
                                            let req = http::Request::from_parts(
                                                parts,
                                                axum::body::Body::new(body),
                                            );
                                            <Router as tower::Service<
                                                http::Request<axum::body::Body>,
                                            >>::call(&mut router, req)
                                            .await
                                        }
                                    },
                                );
                                if let Err(e) = hyper::server::conn::http1::Builder::new()
                                    .serve_connection(io, service)
                                    .await
                                {
                                    log::debug!("[Share] 桥接连接结束: {e}");
                                }
                            });
                        }
                        Err(e) => {
                            log::debug!("[Share] 桥接 accept 失败: {e}");
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        }
                    }
                }
                _ = &mut shutdown_rx => break,
            }
        }
        log::info!("[Share] 消费侧桥接已停止");
    });

    log::info!("[Share] 消费侧桥接启动于 127.0.0.1:{actual_port}");
    Ok(BridgeHandle {
        port: actual_port,
        shutdown: Some(shutdown_tx),
        task: Some(task),
    })
}

/// 桥接入口：/peer/<peer_id>/<原始路径>，或
/// /peer/<peer_id>/provider/<provider_id>/<原始路径>。
async fn bridge_handler(
    State(st): State<BridgeState>,
    req: http::Request<axum::body::Body>,
) -> Response {
    let path = req.uri().path().to_string();
    let rest = path.trim_start_matches('/');
    let Some(rest) = rest.strip_prefix("peer/") else {
        return error_response(
            http::StatusCode::NOT_FOUND,
            "路径须为 /peer/<peer_id>/<api-path>",
        );
    };
    let (peer_id, rest) = match rest.split_once('/') {
        Some((p, sub)) => (p.to_string(), sub),
        None => (rest.to_string(), ""),
    };
    let (provider_id, api_path) = if let Some(provider_rest) = rest.strip_prefix("provider/") {
        let (provider_id, api_rest) = match provider_rest.split_once('/') {
            Some((provider, path)) => (Some(provider.to_string()), path),
            None => (Some(provider_rest.to_string()), ""),
        };
        (provider_id, format!("/{api_rest}"))
    } else {
        (None, format!("/{rest}"))
    };
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let path_and_query = format!("{api_path}{query}");
    let method = req.method().clone();
    let mut headers = req.headers().clone();
    if let Some(provider_id) = provider_id {
        let Ok(value) = http::HeaderValue::from_str(&provider_id) else {
            return error_response(http::StatusCode::BAD_REQUEST, "无效的共享 Provider ID");
        };
        headers.insert(HEADER_ROUTE_PROVIDER, value);
    }
    let body = match axum::body::to_bytes(req.into_body(), 256 * 1024 * 1024).await {
        Ok(b) => b.to_vec(),
        Err(e) => {
            return error_response(
                http::StatusCode::PAYLOAD_TOO_LARGE,
                &format!("读取请求体失败: {e}"),
            )
        }
    };

    st.manager
        .http_via_peer(&peer_id, method, &path_and_query, headers, body)
        .await
        .unwrap_or_else(|e| error_response(http::StatusCode::BAD_GATEWAY, &e))
}

pub fn error_response(status: http::StatusCode, message: &str) -> Response {
    let body = serde_json::json!({
        "error": {
            "type": "tokentap_share_error",
            "message": message,
        }
    });
    let mut resp = Response::new(axum::body::Body::from(body.to_string()));
    *resp.status_mut() = status;
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    resp
}
