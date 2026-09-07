//! OpenAI Official 共享的安全边界。
//!
//! Official 共享只接受显式绑定到本机 Codex OAuth manager 的账号。消费方提供的
//! HTTP 认证头不参与账号选择；受信路由信息由 ingress 在 HMAC 校验后写入请求
//! extensions，并在 forwarder 注入出借方凭据前再次核对。

use crate::database::lend_provider_id;
use crate::provider::Provider;

/// HMAC 准入成功后由出借侧 ingress 创建的内部请求上下文。
///
/// 该类型从不从请求头反序列化，因此消费方无法伪造 `is_shared` 或替换账号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedRequestContext {
    pub peer_id: String,
    pub app_type: String,
    pub provider_id: Option<String>,
}

/// 返回可用于 Official 共享的显式 Codex OAuth 账号。
pub fn managed_codex_official_account_id(app_type: &str, provider: &Provider) -> Option<String> {
    if app_type != "codex"
        || provider.category.as_deref() != Some("official")
        || !crate::proxy::providers::is_codex_official_provider(provider)
    {
        return None;
    }

    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.managed_account_id_for("codex_oauth"))
        .map(|account_id| account_id.trim().to_string())
        .filter(|account_id| !account_id.is_empty())
}

/// 非 Official 供应商保持既有共享规则；Official 仅允许显式托管的 Codex OAuth。
pub fn is_structurally_shareable(app_type: &str, provider: &Provider) -> bool {
    provider.category.as_deref() != Some("official")
        || managed_codex_official_account_id(app_type, provider).is_some()
}

/// 核对共享上下文与 lender 合成 Provider，并返回需要动态注入的账号 ID。
pub fn shared_codex_official_account_id(
    app_type: &str,
    provider: &Provider,
    context: Option<&SharedRequestContext>,
) -> Result<Option<String>, String> {
    let Some(context) = context else {
        return Ok(None);
    };
    if context.app_type != app_type {
        return Err("共享请求应用类型与已认证上下文不一致".to_string());
    }

    let Some(provider_id) = context.provider_id.as_deref() else {
        if provider.category.as_deref() == Some("official") {
            return Err("Official 共享请求必须显式指定 Provider".to_string());
        }
        return Ok(None);
    };
    let expected_lend_id = lend_provider_id(&context.peer_id, provider_id);
    if provider.id != expected_lend_id {
        return Err("共享请求 Provider 与已认证路由目标不一致".to_string());
    }

    if provider.category.as_deref() != Some("official") {
        return Ok(None);
    }
    managed_codex_official_account_id(app_type, provider)
        .map(Some)
        .ok_or_else(|| "该 Official Provider 未显式绑定本机 Codex OAuth 账号".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{AuthBinding, AuthBindingSource, ProviderMeta};
    use serde_json::json;

    fn managed_official() -> Provider {
        let mut provider = Provider::with_id(
            "codex-official".to_string(),
            "OpenAI Official".to_string(),
            json!({ "auth": {}, "config": "" }),
            None,
        );
        provider.category = Some("official".to_string());
        provider.meta = Some(ProviderMeta {
            auth_binding: Some(AuthBinding {
                source: AuthBindingSource::ManagedAccount,
                auth_provider: Some("codex_oauth".to_string()),
                account_id: Some("acct-lender".to_string()),
            }),
            ..Default::default()
        });
        provider
    }

    #[test]
    fn only_managed_codex_official_is_structurally_shareable() {
        let managed = managed_official();
        assert!(is_structurally_shareable("codex", &managed));
        assert!(!is_structurally_shareable("claude", &managed));

        let mut native = managed;
        native.meta = None;
        assert!(!is_structurally_shareable("codex", &native));
    }

    #[test]
    fn shared_context_must_match_lender_provider() {
        let mut provider = managed_official();
        provider.id = lend_provider_id("peer-a", "codex-official");
        let context = SharedRequestContext {
            peer_id: "peer-a".to_string(),
            app_type: "codex".to_string(),
            provider_id: Some("codex-official".to_string()),
        };
        assert_eq!(
            shared_codex_official_account_id("codex", &provider, Some(&context)).unwrap(),
            Some("acct-lender".to_string())
        );

        provider.id = lend_provider_id("peer-b", "codex-official");
        assert!(shared_codex_official_account_id("codex", &provider, Some(&context)).is_err());
    }
}
