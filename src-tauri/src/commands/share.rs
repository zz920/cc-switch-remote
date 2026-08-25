//! 组网（TokenTap Share）相关的 Tauri 命令

use crate::share::keystore::KeyStorage;
use crate::share::types::{
    CreateNetworkResult, RequestJoinResult, ShareNetworkStatus, ShareQuotaConfig,
};
use crate::store::AppState;

/// 创建组网网络，返回 share id 与 share key（key 仅展示一次）
#[tauri::command]
pub async fn share_create_network(
    state: tauri::State<'_, AppState>,
    node_name: Option<String>,
) -> Result<CreateNetworkResult, String> {
    let name = node_name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            // 默认节点名：主机名，退化为“未命名节点”
            hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "未命名节点".to_string())
        });
    state.share_manager.create_network(name).await
}

/// 发起加入（凭 share id），返回 6 位短码等待出借方审批
#[tauri::command]
pub async fn share_request_join(
    state: tauri::State<'_, AppState>,
    share_id: String,
) -> Result<RequestJoinResult, String> {
    state.share_manager.request_join(share_id).await
}

/// 取消等待中的加入申请
#[tauri::command]
pub async fn share_cancel_join(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.share_manager.cancel_join().await
}

/// 出借方批准加入申请
#[tauri::command]
pub async fn share_approve_join(
    state: tauri::State<'_, AppState>,
    peer_id: String,
) -> Result<(), String> {
    state.share_manager.approve_join(&peer_id).await
}

/// 出借方拒绝加入申请
#[tauri::command]
pub async fn share_reject_join(
    state: tauri::State<'_, AppState>,
    peer_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    state.share_manager.reject_join(&peer_id, reason).await
}

/// 退出/解散网络
#[tauri::command]
pub async fn share_leave_network(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.share_manager.leave_network().await
}

/// 获取组网完整状态
#[tauri::command]
pub async fn share_get_status(
    state: tauri::State<'_, AppState>,
) -> Result<ShareNetworkStatus, String> {
    state.share_manager.get_status().await
}

/// 设置出借白名单（"app:provider_id" 列表）
#[tauri::command]
pub async fn share_set_shared_providers(
    state: tauri::State<'_, AppState>,
    provider_ids: Vec<String>,
) -> Result<(), String> {
    state.share_manager.set_shared_providers(provider_ids).await
}

/// 设置出借限额
#[tauri::command]
pub async fn share_set_quota(
    state: tauri::State<'_, AppState>,
    config: ShareQuotaConfig,
) -> Result<(), String> {
    state.share_manager.set_quota(config).await
}

/// 拉黑节点
#[tauri::command]
pub async fn share_block_peer(
    state: tauri::State<'_, AppState>,
    peer_id: String,
    reason: Option<String>,
) -> Result<(), String> {
    state.share_manager.block_peer(&peer_id, reason).await
}

/// 解除拉黑
#[tauri::command]
pub async fn share_unblock_peer(
    state: tauri::State<'_, AppState>,
    peer_id: String,
) -> Result<(), String> {
    state.share_manager.unblock_peer(&peer_id).await
}

/// 重新生成 share key（旧 key 立即失效）
#[tauri::command]
pub async fn share_regenerate_key(state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.share_manager.regenerate_key().await
}

/// 设置消费侧路由偏好（local_only/local_first/network_first/network_only）
#[tauri::command]
pub async fn share_set_route_preference(
    state: tauri::State<'_, AppState>,
    preference: String,
) -> Result<(), String> {
    state.share_manager.set_route_preference(preference).await
}

/// 设置 relay 地址覆盖（None = 恢复官方默认）
#[tauri::command]
pub async fn share_set_relay_addr(
    state: tauri::State<'_, AppState>,
    addr: Option<String>,
) -> Result<(), String> {
    state.share_manager.set_relay_addr(addr).await
}

/// 设置节点显示名
#[tauri::command]
pub async fn share_set_node_name(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<(), String> {
    state.share_manager.set_node_name(name).await
}

/// share key 的存储位置（keyring/file，file 时 UI 提示风险）
#[tauri::command]
pub async fn share_key_storage(state: tauri::State<'_, AppState>) -> Result<KeyStorage, String> {
    Ok(state.share_manager.key_storage().await)
}
