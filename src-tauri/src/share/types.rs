//! 组网（TokenTap Share）前后端共享类型
//!
//! 全部为 serde camelCase，与前端 TypeScript 类型一一对应。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 网络角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareRole {
    /// 网络创建者
    Creator,
    /// 通过审批加入的成员
    Member,
}

impl ShareRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Creator => "creator",
            Self::Member => "member",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "creator" => Self::Creator,
            _ => Self::Member,
        }
    }
}

/// 消费侧路由偏好
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RoutePreference {
    /// 只用本地供应商（未注入远端路由）
    #[default]
    LocalOnly,
    /// 本地优先，远端节点作为故障转移后备
    LocalFirst,
    /// 网络节点优先，本地作为回落
    NetworkFirst,
    /// 仅使用网络节点
    NetworkOnly,
}

impl RoutePreference {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::LocalOnly => "local_only",
            Self::LocalFirst => "local_first",
            Self::NetworkFirst => "network_first",
            Self::NetworkOnly => "network_only",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "local_first" => Self::LocalFirst,
            "network_first" => Self::NetworkFirst,
            "network_only" => Self::NetworkOnly,
            _ => Self::LocalOnly,
        }
    }
}

/// 单个 peer 的状态（展示用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderInfo {
    /// Provider 所属应用（codex/claude/gemini/grokbuild）
    pub app: String,
    /// 出借方本地 Provider ID
    pub provider_id: String,
    /// 出借方 Provider 显示名称
    pub name: String,
    /// 该 Provider 声明的可用模型
    pub models: Vec<String>,
}

/// 共享 Provider 的连通性检查结果（由出借方执行探测，不发送实际模型请求）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderCheckResult {
    pub success: bool,
    pub status: String,
    pub message: String,
    pub response_time_ms: Option<u64>,
    pub http_status: Option<u16>,
    pub tested_at: i64,
}

/// 单个 peer 的状态（展示用）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePeerInfo {
    pub peer_id: String,
    /// 节点显示名（能力通告获得，可能为空）
    pub name: String,
    pub online: bool,
    /// 是否 P2P 直连（false = 经中继）
    pub direct: bool,
    /// 该节点共享出来的应用类型（claude/codex/gemini...）
    pub shared_apps: Vec<String>,
    /// 该节点当前实际共享的 Provider 与模型摘要（不包含密钥/配置）
    pub providers: Vec<ShareProviderInfo>,
    /// 本周期该 peer 已消耗的 token（出借侧视角；消费侧为 0）
    pub tokens_used: i64,
    /// 剩余配额（出借侧视角；无限额为 None）
    pub quota_remaining: Option<i64>,
    pub is_blocked: bool,
}

/// 加入请求（出借方待审批）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRequestInfo {
    pub peer_id: String,
    pub node_name: String,
    /// 6 位短码，需与邀请人带外核对
    pub short_code: String,
    pub received_at: i64,
}

/// 等待审批状态（加入方视角）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingJoinInfo {
    pub share_id: String,
    pub short_code: String,
    pub expires_at: i64,
}

/// 组网网络完整状态（share_get_status 返回）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShareNetworkStatus {
    pub joined: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub route_preference: String,
    /// 消费侧按应用选择的远端 Provider target。缺少某应用键表示全部可用 Provider。
    pub route_targets: HashMap<String, Vec<String>>,
    pub node_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_addr: Option<String>,
    /// 出借白名单（"app:provider_id" 列表）
    pub shared_provider_ids: Vec<String>,
    pub quota_scope: String,
    pub quota_max_tokens: i64,
    pub quota_per_peer: bool,
    /// 当前配额周期内，本机供应商向网络其他节点提供的 token 总量
    pub provided_tokens: i64,
    /// 当前配额周期内，本机通过网络其他节点消费的 token 总量
    pub consumed_tokens: i64,
    pub peers: Vec<SharePeerInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_join: Option<PendingJoinInfo>,
    /// 出借方收到的待审批加入请求
    pub incoming_requests: Vec<JoinRequestInfo>,
    /// 消费侧本地桥接是否在运行
    pub bridge_running: bool,
    /// relay 控制连接是否已建立并完成预约
    pub relay_connected: bool,
    /// 当前 relay bootstrap transport（quic/tcp）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_transport: Option<String>,
    /// 本机 PeerId
    pub local_peer_id: String,
}

/// 创建网络的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNetworkResult {
    pub share_id: String,
    /// 仅创建时展示一次（用于带外备份）；分享链接不含 key
    pub share_key: String,
}

/// 发起加入的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestJoinResult {
    pub short_code: String,
    pub expires_at: i64,
}

/// 限额配置（share_set_quota 入参）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareQuotaConfig {
    /// daily / monthly
    pub scope: String,
    /// token 上限，0 = 不限
    pub max_tokens: i64,
    /// 是否按 peer 分别限额
    pub per_peer: bool,
}
