import {
  Activity,
  BarChart3,
  Check,
  Copy,
  Edit,
  GripVertical,
  Loader2,
  Network,
  Play,
  RefreshCw,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import type { TFunction } from "i18next";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import type { ProviderDragHandleProps } from "@/components/providers/ProviderCard";
import { getAppLabel, isProxyAppId } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import { proxyKeys } from "@/lib/query/proxy";
import {
  useActivateSharedProvider,
  useShareStatus,
  useTestSharedProvider,
} from "@/lib/query/share";
import { cn } from "@/lib/utils";
import { extractErrorMessage } from "@/utils/errorUtils";

interface ShareProviderRouteCardProps {
  appId: AppId;
  isProxyRunning: boolean;
  isProxyTakeover: boolean;
  onOpenShareSettings?: () => void;
  dragHandleProps?: ProviderDragHandleProps;
}

interface ProviderEntry {
  targetId: string;
  peerId: string;
  providerId: string;
  name: string;
  /** 出借该供应商的节点名（同名供应商多节点出借时用于区分） */
  nodeName: string;
  /** 本周期经该节点此供应商消费的 token（消费方本地统计） */
  usedTokens: number | null;
  /** 出借方为本节点配置的配额上限（节点级；无配额为 null） */
  peerQuotaCap: number | null;
  models: string[];
  defaultModel?: string | null;
  /** 托管 OAuth（OpenAI Official）：模型由账号动态决定，通告不带模型。 */
  isManagedOauth: boolean;
  coverage: Record<string, number>;
}

function formatRefreshTime(timestamp: number, t: TFunction) {
  const minutes = Math.floor(Math.max(0, Date.now() - timestamp) / 60_000);
  if (minutes < 1) {
    return t("share.networkProvider.justNow", { defaultValue: "刚刚" });
  }
  return t("share.networkProvider.minutesAgo", {
    count: minutes,
    defaultValue: `${minutes} 分钟前`,
  });
}

function DisabledAction({
  icon: Icon,
  label,
}: {
  icon: LucideIcon;
  label: string;
}) {
  return (
    <span className="inline-flex cursor-not-allowed" title={label}>
      <Button
        size="icon"
        variant="ghost"
        disabled
        aria-label={label}
        className="h-8 w-8 cursor-not-allowed p-1 text-muted-foreground opacity-40"
      >
        <Icon className="h-4 w-4" />
      </Button>
    </span>
  );
}

/** 用户模式下作为当前 Agent Provider 列表中的一个可选共享路由。 */
export function ShareProviderRouteCard({
  appId,
  isProxyRunning,
  isProxyTakeover,
  onOpenShareSettings,
  dragHandleProps,
}: ShareProviderRouteCardProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: status, refetch, isFetching, dataUpdatedAt } = useShareStatus();
  const activateProvider = useActivateSharedProvider();
  const testProvider = useTestSharedProvider();
  const [lastCheck, setLastCheck] = useState<
    { target: string; message: string; success: boolean } | undefined
  >();

  const entries = useMemo<ProviderEntry[]>(() => {
    if (!status) return [];
    const onlinePeers = (status.peers ?? []).filter(
      (peer) => peer.online && !peer.isBlocked,
    );
    const coverage = new Map<string, Set<string>>();
    for (const peer of onlinePeers) {
      for (const provider of peer.providers ?? []) {
        if (provider.app !== appId) continue;
        for (const model of provider.models ?? []) {
          const peers = coverage.get(model) ?? new Set<string>();
          peers.add(peer.peerId);
          coverage.set(model, peers);
        }
      }
    }
    return onlinePeers.flatMap((peer) =>
      (peer.providers ?? [])
        .filter((provider) => provider.app === appId)
        .map((provider) => ({
          targetId: `${peer.peerId}:${provider.providerId}`,
          peerId: peer.peerId,
          providerId: provider.providerId,
          name: provider.name,
          nodeName: peer.name,
          usedTokens: provider.usedTokens ?? null,
          peerQuotaCap:
            (peer.tokensUsed ?? 0) + (peer.quotaRemaining ?? 0) || null,
          models: provider.models ?? [],
          defaultModel: provider.defaultModel,
          isManagedOauth: provider.authMode === "managed_oauth",
          coverage: Object.fromEntries(
            (provider.models ?? []).map((model) => [
              model,
              coverage.get(model)?.size ?? 0,
            ]),
          ),
        })),
    );
  }, [appId, status]);

  if (!isProxyAppId(appId) || !status?.joined || status.mode !== "consumer") {
    return null;
  }

  const selectedTarget = status.routeTargets?.[appId]?.[0];
  const selectedEntry = entries.find(
    (entry) => entry.targetId === selectedTarget,
  );
  const actionEntry = selectedEntry ?? entries[0];
  const isCurrent = Boolean(selectedTarget);
  const totalNodes = (status.peers ?? []).filter(
    (peer) => peer.online && !peer.isBlocked,
  ).length;

  const activateEntry = async (entry: ProviderEntry) => {
    if (activateProvider.isPending || selectedTarget === entry.targetId) return;
    try {
      await activateProvider.mutateAsync({
        appType: appId,
        peerId: entry.peerId,
        providerId: entry.providerId,
      });
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: proxyKeys.status }),
        queryClient.invalidateQueries({ queryKey: proxyKeys.takeoverStatus }),
        queryClient.invalidateQueries({ queryKey: ["providers", appId] }),
      ]);
      toast.success(
        t("share.networkProvider.enabledRestartRequired", {
          app: getAppLabel(appId),
          defaultValue:
            "已启用共享网络并更新 {{app}} 配置，请重启客户端以加载新的路由。",
        }),
        { closeButton: true },
      );
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  const checkEntry = async (entry: ProviderEntry) => {
    try {
      const result = await testProvider.mutateAsync({
        appType: appId,
        peerId: entry.peerId,
        providerId: entry.providerId,
      });
      setLastCheck({
        target: entry.targetId,
        message: result.message,
        success: result.success,
      });
      result.success
        ? toast.success(result.message)
        : toast.error(result.message);
    } catch (error) {
      const message = extractErrorMessage(error);
      setLastCheck({ target: entry.targetId, message, success: false });
      toast.error(message);
    }
  };

  const actionPending = activateProvider.isPending;
  const actionsDisabled = !actionEntry;

  return (
    <section
      className={cn(
        "group relative overflow-hidden rounded-xl border border-border bg-card p-4 text-card-foreground transition-all duration-300",
        "hover:border-emerald-500/50 hover:shadow-sm",
        isCurrent && "border-emerald-500/60 shadow-sm shadow-emerald-500/10",
        dragHandleProps?.isDragging &&
          "z-10 scale-105 cursor-grabbing border-primary shadow-lg",
      )}
    >
      <div
        className={cn(
          "pointer-events-none absolute inset-0 bg-gradient-to-r from-emerald-500/10 to-transparent transition-opacity duration-500",
          isCurrent ? "opacity-100" : "opacity-0",
        )}
      />

      <div className="relative flex items-center gap-3">
        <div className="flex min-w-0 flex-1 items-center gap-2">
          {dragHandleProps && (
            <button
              type="button"
              className={cn(
                "-ml-1.5 flex-shrink-0 cursor-grab p-1.5 active:cursor-grabbing",
                "text-muted-foreground/50 transition-colors hover:text-muted-foreground",
                dragHandleProps.isDragging && "cursor-grabbing",
              )}
              aria-label={t("provider.dragHandle")}
              title={t("provider.dragHandle")}
              {...dragHandleProps.attributes}
              {...dragHandleProps.listeners}
            >
              <GripVertical className="h-4 w-4" />
            </button>
          )}

          <div className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-lg border border-border bg-muted transition-transform duration-300 group-hover:scale-105">
            <Network className="h-5 w-5 text-emerald-500" />
          </div>
          <div className="min-w-0 flex-1 space-y-1">
            <h3 className="text-base font-semibold leading-none">
              {t("share.networkProvider.title", { defaultValue: "共享网络" })}
            </h3>
            <p className="truncate text-sm text-muted-foreground">
              {t("share.networkProvider.description", {
                app: getAppLabel(appId),
                defaultValue:
                  "从网络节点选择一个 {{app}} Provider；本地 Provider 仍可随时切换。",
              })}
            </p>
          </div>
        </div>

        <div className="pointer-events-none ml-auto flex flex-shrink-0 items-center gap-1.5 opacity-0 transition-opacity duration-200 group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100">
          <Button
            size="sm"
            variant={isCurrent ? "secondary" : "default"}
            onClick={() => actionEntry && void activateEntry(actionEntry)}
            disabled={actionsDisabled || actionPending || isCurrent}
            className={cn(
              "w-[4.5rem] px-2.5",
              isCurrent
                ? "bg-gray-200 text-muted-foreground hover:bg-gray-200 hover:text-muted-foreground dark:bg-gray-700 dark:hover:bg-gray-700"
                : "bg-emerald-500 hover:bg-emerald-600 dark:bg-emerald-600 dark:hover:bg-emerald-700",
            )}
          >
            {actionPending ? (
              <Loader2 className="h-4 w-4 animate-spin" />
            ) : isCurrent ? (
              <Check className="h-4 w-4" />
            ) : (
              <Play className="h-4 w-4" />
            )}
            {isCurrent
              ? t("provider.inUse", { defaultValue: "使用中" })
              : t("provider.enable", { defaultValue: "启用" })}
          </Button>

          <div className="flex items-center gap-1">
            <DisabledAction
              icon={Edit}
              label={t("share.networkProvider.editUnavailable", {
                defaultValue: "共享 Provider 由提供方管理，不能在本机编辑",
              })}
            />
            <DisabledAction
              icon={Copy}
              label={t("share.networkProvider.duplicateUnavailable", {
                defaultValue: "共享 Provider 不能复制为本地配置",
              })}
            />
            <Button
              size="icon"
              variant="ghost"
              onClick={() => actionEntry && void checkEntry(actionEntry)}
              disabled={actionsDisabled || testProvider.isPending}
              aria-label={t("provider.connectivityCheck", {
                defaultValue: "检测连通",
              })}
              title={t("provider.connectivityCheck", {
                defaultValue: "检测连通",
              })}
              className="h-8 w-8 p-1"
            >
              {testProvider.isPending ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Activity className="h-4 w-4" />
              )}
            </Button>
            <Button
              size="icon"
              variant="ghost"
              onClick={onOpenShareSettings}
              disabled={!onOpenShareSettings}
              aria-label={t("provider.configureUsage", {
                defaultValue: "配置用量查询",
              })}
              title={t("provider.configureUsage", {
                defaultValue: "配置用量查询",
              })}
              className="h-8 w-8 p-1"
            >
              <BarChart3 className="h-4 w-4" />
            </Button>
            <DisabledAction
              icon={Trash2}
              label={t("share.networkProvider.deleteUnavailable", {
                defaultValue: "共享 Provider 不能在本机删除",
              })}
            />
          </div>
        </div>
      </div>

      <div className="relative mt-4 overflow-hidden rounded-lg border border-border/70 bg-background/60">
        <div className="flex items-center justify-between gap-3 border-b border-border/60 px-3 py-2.5">
          <div className="min-w-0">
            <h4 className="text-sm font-medium">
              {t("share.networkProvider.listTitle", {
                defaultValue: "共享网络供应商",
              })}
            </h4>
            <p className="text-xs text-muted-foreground">
              {getAppLabel(appId)} ·{" "}
              {t("share.networkProvider.providerCount", {
                count: entries.length,
                defaultValue: `${entries.length} 个 Provider`,
              })}
            </p>
          </div>
          <div className="flex shrink-0 items-center gap-1 text-xs text-muted-foreground">
            <span>{formatRefreshTime(dataUpdatedAt || Date.now(), t)}</span>
            <Button
              size="icon"
              variant="ghost"
              onClick={() => void refetch()}
              disabled={isFetching}
              aria-label={t("share.networkProvider.refresh", {
                defaultValue: "刷新共享 Provider",
              })}
              title={t("share.networkProvider.refresh", {
                defaultValue: "刷新共享 Provider",
              })}
              className="h-8 w-8"
            >
              <RefreshCw
                className={isFetching ? "h-4 w-4 animate-spin" : "h-4 w-4"}
              />
            </Button>
          </div>
        </div>

        <div className="space-y-1 p-2">
          {entries.length === 0 ? (
            <p className="rounded-md border border-dashed border-border px-3 py-4 text-sm text-muted-foreground">
              {t("share.networkProvider.noneAvailable", {
                app: getAppLabel(appId),
                defaultValue: "当前没有在线节点为 {{app}} 提供可用 Provider。",
              })}
            </p>
          ) : (
            entries.map((entry) => {
              const active = selectedTarget === entry.targetId;
              const displayModel = entry.isManagedOauth
                ? t("share.networkProvider.managedOauthModel", {
                    defaultValue: "ChatGPT 账号动态模型",
                  })
                : (entry.defaultModel ??
                  entry.models[0] ??
                  t("share.networkProvider.modelsUnknown", {
                    defaultValue: "模型信息待刷新",
                  }));
              // 托管 OAuth 不按模型统计覆盖率（通告无模型），节点在线即可用。
              const coverage = entry.isManagedOauth
                ? 1
                : entry.models.reduce(
                    (max, model) => Math.max(max, entry.coverage[model] ?? 0),
                    0,
                  );
              return (
                <div
                  key={entry.targetId}
                  className="rounded-md px-2 py-2 transition-colors hover:bg-muted/40"
                >
                  <div className="flex items-center gap-3">
                    <label className="flex min-w-0 flex-1 cursor-pointer items-start gap-2">
                      <input
                        type="radio"
                        name={`share-provider-${appId}`}
                        checked={active}
                        onChange={() => void activateEntry(entry)}
                        disabled={activateProvider.isPending}
                        className="mt-1 h-4 w-4 accent-primary"
                        aria-label={`${entry.name} ${displayModel}`}
                      />
                      <span className="min-w-0">
                        <span className="block truncate text-sm font-medium">
                          {entry.name}
                          {entry.nodeName && (
                            <span className="ml-1 font-normal text-muted-foreground">
                              @{entry.nodeName}
                            </span>
                          )}
                        </span>
                        <span className="block truncate text-xs text-muted-foreground">
                          {displayModel}
                        </span>
                      </span>
                    </label>
                    <div className="flex shrink-0 items-center gap-x-2 text-right text-[10px] text-muted-foreground sm:text-[11px]">
                      {entry.peerQuotaCap && entry.peerQuotaCap > 0 ? (
                        <span
                          title={t("share.networkProvider.quotaIsPeerLevel", {
                            defaultValue:
                              "配额为节点级，供应商行的用量为其归属节点的占用",
                          })}
                        >
                          {t("share.networkProvider.total", {
                            defaultValue: "总: {{total}}",
                            total: entry.peerQuotaCap.toLocaleString(),
                          })}
                        </span>
                      ) : null}
                      {entry.peerQuotaCap && entry.peerQuotaCap > 0 ? (
                        <span aria-hidden="true">|</span>
                      ) : null}
                      <span>
                        {t("share.networkProvider.used", {
                          defaultValue: "已使用: {{used}}",
                          used: (entry.usedTokens ?? 0).toLocaleString(),
                        })}
                      </span>
                      <span aria-hidden="true">|</span>
                      <span>
                        {coverage}/{totalNodes}
                      </span>
                    </div>
                  </div>
                  {lastCheck?.target === entry.targetId && (
                    <span
                      role="status"
                      className={cn(
                        "ml-6 mt-1 block max-w-full truncate text-xs",
                        lastCheck.success
                          ? "text-emerald-600"
                          : "text-destructive",
                      )}
                      title={lastCheck.message}
                    >
                      {lastCheck.message}
                    </span>
                  )}
                </div>
              );
            })
          )}
        </div>
      </div>

      {(!isProxyRunning || !isProxyTakeover) && isCurrent && (
        <p className="relative mt-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-200">
          {t("share.networkProvider.proxyStarting", {
            app: getAppLabel(appId),
            defaultValue:
              "正在为 {{app}} 启用本地路由接管；请等待状态刷新后重启客户端。",
          })}
        </p>
      )}
    </section>
  );
}
