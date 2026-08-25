//! 组网（TokenTap Share）相关类型，与后端 serde camelCase 对齐

/** 节点信息 */
export interface SharePeer {
  peerId: string;
  name: string;
  online: boolean;
  /** 是否 P2P 直连（false = 经中继） */
  direct: boolean;
  /** 该节点共享的应用类型 */
  sharedApps: string[];
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
  nodeName: string;
  relayAddr?: string;
  sharedProviderIds: string[];
  quotaScope: string;
  quotaMaxTokens: number;
  quotaPerPeer: boolean;
  peers: SharePeer[];
  pendingJoin?: PendingJoin;
  incomingRequests: JoinRequest[];
  bridgeRunning: boolean;
  localPeerId: string;
}

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
