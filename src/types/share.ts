//! 组网（TokenTap Share）相关类型，与后端 serde camelCase 对齐

/** 出借方公开的 Provider 能力摘要（不含密钥） */
export interface ShareProviderInfo {
  app: string;
  providerId: string;
  name: string;
  models: string[];
  /** Provider 当前配置的默认模型；旧版节点可能不通告。 */
  defaultModel?: string | null;
}

/** 节点信息 */
export interface SharePeer {
  peerId: string;
  name: string;
  online: boolean;
  /** 是否 P2P 直连（false = 经中继） */
  direct: boolean;
  /** 该节点共享的应用类型 */
  sharedApps: string[];
  providers: ShareProviderInfo[];
  tokensUsed: number;
  quotaRemaining: number | null;
  isBlocked: boolean;
}

/** 加入申请（出借方待审批） */
export interface JoinRequest {
  peerId: string;
  nodeName: string;
  shortCode: string;
  receivedAt: number;
}

/** 等待审批状态（加入方视角） */
export interface PendingJoin {
  shareId: string;
  shortCode: string;
  expiresAt: number;
}

/** 组网网络完整状态 */
export interface ShareStatus {
  joined: boolean;
  shareId?: string;
  role?: string;
  routePreference: string;
  mode: ShareMode;
  /** 按应用选择的远端 Provider target；缺少应用键表示使用本地 Provider */
  routeTargets: Record<string, string[]>;
  nodeName: string;
  relayAddr?: string;
  sharedProviderIds: string[];
  quotaScope: string;
  quotaMaxTokens: number;
  quotaPerPeer: boolean;
  /** 当前配额周期内，本机向共享网络提供的 token */
  providedTokens: number;
  /** 当前配额周期内，本机从共享网络消费的 token */
  consumedTokens: number;
  peers: SharePeer[];
  pendingJoin?: PendingJoin;
  incomingRequests: JoinRequest[];
  bridgeRunning: boolean;
  relayConnected: boolean;
  relayTransport?: string;
  localPeerId: string;
}

export type ShareMode = "provider" | "consumer";

/** 创建网络结果 */
export interface CreateNetworkResult {
  shareId: string;
  shareKey: string;
}

/** 发起加入结果 */
export interface RequestJoinResult {
  shortCode: string;
  expiresAt: number;
}

/** 限额配置 */
export interface ShareQuotaConfig {
  scope: string;
  maxTokens: number;
  perPeer: boolean;
}

/** share key 存储位置 */
export type ShareKeyStorage = "keyring" | "file";

/** 路由偏好 */
export type ShareRoutePreference =
  | "local_only"
  | "local_first"
  | "network_first"
  | "network_only";

/** 共享 Provider 连通性探测结果 */
export interface ShareProviderCheckResult {
  success: boolean;
  status: string;
  message: string;
  responseTimeMs?: number;
  httpStatus?: number;
  testedAt: number;
}
