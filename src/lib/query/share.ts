import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { shareApi } from "@/lib/api/share";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type {
  ShareQuotaConfig,
  ShareProviderCheckResult,
  ShareRoutePreference,
  ShareStatus,
} from "@/types/share";

export const shareKeys = {
  status: ["shareStatus"] as const,
  keyStorage: ["shareKeyStorage"] as const,
};

/** 轮询间隔：10 秒 */
const SHARE_STATUS_POLL_INTERVAL = 10_000;

/**
 * 获取组网状态（10s 轮询）
 */
export function useShareStatus(poll = true) {
  const queryClient = useQueryClient();
  useTauriEvent("share:join-request", () => {
    void queryClient.invalidateQueries({ queryKey: shareKeys.status });
  });
  useTauriEvent("share:join-resolved", () => {
    void queryClient.invalidateQueries({ queryKey: shareKeys.status });
  });

  return useQuery({
    queryKey: shareKeys.status,
    queryFn: () => shareApi.getStatus(),
    refetchInterval: poll ? SHARE_STATUS_POLL_INTERVAL : false,
    placeholderData: (previousData: ShareStatus | undefined) => previousData,
  });
}

/**
 * 获取 share key 存储位置（file 时 UI 提示风险）
 */
export function useShareKeyStorage() {
  return useQuery({
    queryKey: shareKeys.keyStorage,
    queryFn: () => shareApi.keyStorage(),
  });
}

/** mutation 成功后统一失效组网相关查询 */
function useShareMutation<TVariables, TResult>(
  mutationFn: (variables: TVariables) => Promise<TResult>,
) {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn,
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: shareKeys.status });
    },
  });
}

// ========== 网络生命周期 ==========

export function useCreateNetwork() {
  return useShareMutation((nodeName?: string) =>
    shareApi.createNetwork(nodeName),
  );
}

export function useRequestJoin() {
  return useShareMutation((shareId: string) => shareApi.requestJoin(shareId));
}

export function useCancelJoin() {
  return useShareMutation((_: void) => shareApi.cancelJoin());
}

export function useLeaveNetwork() {
  return useShareMutation((_: void) => shareApi.leaveNetwork());
}

// ========== 审批（出借方） ==========

export function useApproveJoin() {
  return useShareMutation((peerId: string) => shareApi.approveJoin(peerId));
}

export function useRejectJoin() {
  return useShareMutation((peerId: string) => shareApi.rejectJoin(peerId));
}

// ========== 出借方控制 ==========

export function useSetSharedProviders() {
  return useShareMutation((providerIds: string[]) =>
    shareApi.setSharedProviders(providerIds),
  );
}

export function useSetQuota() {
  return useShareMutation((config: ShareQuotaConfig) =>
    shareApi.setQuota(config),
  );
}

export function useBlockPeer() {
  return useShareMutation((peerId: string) => shareApi.blockPeer(peerId));
}

export function useUnblockPeer() {
  return useShareMutation((peerId: string) => shareApi.unblockPeer(peerId));
}

export function useRegenerateKey() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => shareApi.regenerateKey(),
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: shareKeys.status });
      queryClient.invalidateQueries({ queryKey: shareKeys.keyStorage });
    },
  });
}

// ========== 配置 ==========

export function useSetRoutePreference() {
  return useShareMutation((preference: ShareRoutePreference) =>
    shareApi.setRoutePreference(preference),
  );
}

export function useSetRouteMode() {
  return useShareMutation((mode: "provider" | "consumer") =>
    shareApi.setRouteMode(mode),
  );
}

export function useSetRouteTargets() {
  return useShareMutation(
    ({ appType, targets }: { appType: string; targets: string[] }) =>
      shareApi.setRouteTargets(appType, targets),
  );
}

export function useActivateSharedProvider() {
  return useShareMutation(
    ({
      appType,
      peerId,
      providerId,
    }: {
      appType: string;
      peerId: string;
      providerId: string;
    }) => shareApi.activateProvider(appType, peerId, providerId),
  );
}

export function useTestSharedProvider() {
  return useMutation<
    ShareProviderCheckResult,
    Error,
    { appType: string; peerId: string; providerId: string }
  >({
    mutationFn: ({ appType, peerId, providerId }) =>
      shareApi.testProvider(appType, peerId, providerId),
  });
}

export function useSetRelayAddr() {
  return useShareMutation((addr: string | null) => shareApi.setRelayAddr(addr));
}

export function useSetNodeName() {
  return useShareMutation((name: string) => shareApi.setNodeName(name));
}
