import { invoke } from "@tauri-apps/api/core";
import type {
  CreateNetworkResult,
  RequestJoinResult,
  ShareKeyStorage,
  ShareQuotaConfig,
  ShareProviderCheckResult,
  ShareStatus,
} from "@/types/share";

/** 组网（TokenTap Share）API */
export const shareApi = {
  // ========== 网络生命周期 ==========

  /** 创建网络（返回 share id 与仅展示一次的 share key） */
  async createNetwork(nodeName?: string): Promise<CreateNetworkResult> {
    return invoke("share_create_network", { nodeName: nodeName ?? null });
  },

  /** 发起加入（返回 6 位短码，等待出借方审批） */
  async requestJoin(shareId: string): Promise<RequestJoinResult> {
    return invoke("share_request_join", { shareId });
  },

  /** 取消等待中的加入申请 */
  async cancelJoin(): Promise<void> {
    return invoke("share_cancel_join");
  },

  /** 退出/解散网络 */
  async leaveNetwork(): Promise<void> {
    return invoke("share_leave_network");
  },

  // ========== 审批（出借方） ==========

  async approveJoin(peerId: string): Promise<void> {
    return invoke("share_approve_join", { peerId });
  },

  async rejectJoin(peerId: string, reason?: string): Promise<void> {
    return invoke("share_reject_join", { peerId, reason: reason ?? null });
  },

  // ========== 状态 ==========

  async getStatus(): Promise<ShareStatus> {
    return invoke("share_get_status");
  },

  /** share key 存储位置（file 时 UI 提示风险） */
  async keyStorage(): Promise<ShareKeyStorage> {
    return invoke("share_key_storage");
  },

  // ========== 出借方控制 ==========

  /** 设置出借白名单（"app:provider_id" 列表） */
  async setSharedProviders(providerIds: string[]): Promise<void> {
    return invoke("share_set_shared_providers", { providerIds });
  },

  /** 设置限额 */
  async setQuota(config: ShareQuotaConfig): Promise<void> {
    return invoke("share_set_quota", { config });
  },

  /** 拉黑节点 */
  async blockPeer(peerId: string, reason?: string): Promise<void> {
    return invoke("share_block_peer", { peerId, reason: reason ?? null });
  },

  /** 解除拉黑 */
  async unblockPeer(peerId: string): Promise<void> {
    return invoke("share_unblock_peer", { peerId });
  },

  /** 重新生成 share key（旧 key 立即失效） */
  async regenerateKey(): Promise<string> {
    return invoke("share_regenerate_key");
  },

  // ========== 配置 ==========

  /** 设置消费侧路由偏好 */
  async setRoutePreference(preference: string): Promise<void> {
    return invoke("share_set_route_preference", { preference });
  },

  async setRouteMode(mode: "provider" | "consumer"): Promise<void> {
    return invoke("share_set_route_mode", { mode });
  },

  /** 设置某个应用实际参与路由的共享 Provider target */
  async setRouteTargets(appType: string, targets: string[]): Promise<void> {
    return invoke("share_set_route_targets", { appType, targets });
  },

  /** 启用共享 Provider，并同步 Agent live 配置与本地代理接管。 */
  async activateProvider(
    appType: string,
    peerId: string,
    providerId: string,
  ): Promise<void> {
    return invoke("share_activate_provider", {
      appType,
      peerId,
      providerId,
    });
  },

  /** 检测远端共享 Provider 连通性 */
  async testProvider(
    appType: string,
    peerId: string,
    providerId: string,
  ): Promise<ShareProviderCheckResult> {
    return invoke("share_test_provider", { appType, peerId, providerId });
  },

  /** 设置 relay 地址（null = 恢复官方默认） */
  async setRelayAddr(addr: string | null): Promise<void> {
    return invoke("share_set_relay_addr", { addr });
  },

  /** 设置节点显示名 */
  async setNodeName(name: string): Promise<void> {
    return invoke("share_set_node_name", { name });
  },
};
