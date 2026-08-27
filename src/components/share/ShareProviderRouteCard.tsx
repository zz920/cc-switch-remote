import {
  Activity,
  BarChart3,
  GripVertical,
  Loader2,
  Network,
  RefreshCw,
} from "lucide-react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { getAppLabel, isProxyAppId } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import {
  useSetRoutePreference,
  useSetRouteTargets,
  useShareStatus,
  useTestSharedProvider,
} from "@/lib/query/share";
import { extractErrorMessage } from "@/utils/errorUtils";

interface ShareProviderRouteCardProps {
  appId: AppId;
  isProxyRunning: boolean;
  isProxyTakeover: boolean;
  onOpenShareSettings?: () => void;
  onOpenRoutingSettings?: () => void;
}

interface ProviderEntry {
  targetId: string;
  peerId: string;
  providerId: string;
  name: string;
  models: string[];
  coverage: Record<string, number>;
}

/** 共享网络 Provider：用户模式下作为本地 Provider 列表中的一个可选路由。 */
export function ShareProviderRouteCard({
  appId,
  isProxyRunning,
  isProxyTakeover,
  onOpenShareSettings,
}: ShareProviderRouteCardProps) {
  const { t } = useTranslation();
  const { data: status, refetch, isFetching } = useShareStatus();
  const setRoutePreference = useSetRoutePreference();
  const setRouteTargets = useSetRouteTargets();
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
          models: provider.models ?? [],
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

  const configuredTargets = status.routeTargets?.[appId] ?? [];
  const selectedTarget = configuredTargets[0];
  const totalNodes = (status.peers ?? []).filter(
    (peer) => peer.online && !peer.isBlocked,
  ).length;

  const useEntry = async (entry: ProviderEntry) => {
    try {
      await setRouteTargets.mutateAsync({
        appType: appId,
        targets: [entry.targetId],
      });
      await setRoutePreference.mutateAsync("network_only");
      toast.success(
        t("share.networkProvider.saved", {
          defaultValue: "共享网络路由已更新",
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

  return (
    <section className="overflow-hidden rounded-xl border border-blue-500/25 bg-blue-500/5 shadow-sm">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border/50 px-4 py-3">
        <div className="flex min-w-0 items-center gap-3">
          <Network className="h-5 w-5 shrink-0 text-blue-500" />
          <div className="min-w-0">
            <h3 className="font-semibold">
              {t("share.networkProvider.title", { defaultValue: "共享网络" })}
            </h3>
            <p className="text-xs text-muted-foreground">
              {t("share.networkProvider.description", {
                app: getAppLabel(appId),
                defaultValue:
                  "用户模式可在本地 Provider 与网络节点提供的 {{app}} Provider 之间切换。",
              })}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Badge variant={selectedTarget ? "default" : "secondary"}>
            {selectedTarget
              ? t("share.networkProvider.active", { defaultValue: "正在使用" })
              : t("share.networkProvider.waiting", { defaultValue: "请选择" })}
          </Badge>
          <Button
            size="icon"
            variant="ghost"
            onClick={() => void refetch()}
            disabled={isFetching}
            aria-label={t("share.networkProvider.refresh", {
              defaultValue: "刷新共享网络",
            })}
          >
            <RefreshCw
              className={isFetching ? "h-4 w-4 animate-spin" : "h-4 w-4"}
            />
          </Button>
        </div>
      </div>

      <div className="space-y-2 p-4">
        {entries.length === 0 ? (
          <p className="rounded-lg border border-dashed border-border px-3 py-4 text-sm text-muted-foreground">
            {t("share.networkProvider.noneAvailable", {
              app: getAppLabel(appId),
              defaultValue: "当前没有在线节点为 {{app}} 提供可用 Provider。",
            })}
          </p>
        ) : (
          entries.map((entry) => {
            const active = selectedTarget === entry.targetId;
            return (
              <div
                key={entry.targetId}
                draggable
                className="group flex items-center gap-2 rounded-lg border border-border bg-background/70 px-2 py-2 transition-colors hover:border-primary/40"
              >
                <div
                  className="flex h-10 w-6 shrink-0 cursor-grab items-center justify-center text-muted-foreground active:cursor-grabbing"
                  title={t("share.networkProvider.dragHint", {
                    defaultValue: "拖动调整共享 Provider 顺序",
                  })}
                  aria-label={t("share.networkProvider.dragHint", {
                    defaultValue: "拖动调整共享 Provider 顺序",
                  })}
                >
                  <GripVertical className="h-4 w-4" />
                </div>
                <label className="flex min-w-0 flex-1 cursor-pointer items-start gap-2">
                  <input
                    type="radio"
                    name={`share-provider-${appId}`}
                    checked={active}
                    onChange={() => void useEntry(entry)}
                    className="mt-1 h-4 w-4 accent-primary"
                    aria-label={`${entry.name} ${entry.models.join(", ")}`}
                  />
                  <span className="min-w-0">
                    <span className="block truncate text-sm font-medium">
                      {entry.name}
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {entry.models.length > 0
                        ? entry.models.join(" · ")
                        : t("share.networkProvider.modelsUnknown", {
                            defaultValue: "模型信息待刷新",
                          })}
                    </span>
                  </span>
                </label>
                <div className="flex shrink-0 flex-wrap items-center justify-end gap-x-2 text-right text-[10px] text-muted-foreground sm:text-[11px]">
                  <span>
                    {t("share.networkProvider.total", {
                      defaultValue: "总: 0",
                    })}
                  </span>
                  <span aria-hidden="true">|</span>
                  <span>
                    {t("share.networkProvider.used", {
                      defaultValue: "已使用: 0",
                    })}
                  </span>
                  <span aria-hidden="true">|</span>
                  <span>
                    {entry.models.length > 0
                      ? `${entry.coverage[entry.models[0]] ?? 0}/${totalNodes}`
                      : `0/${totalNodes}`}
                  </span>
                </div>
                <div className="flex shrink-0 items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
                  <Button
                    size="sm"
                    variant={active ? "default" : "ghost"}
                    onClick={() => void useEntry(entry)}
                    disabled={
                      setRouteTargets.isPending || setRoutePreference.isPending
                    }
                  >
                    {t("share.networkProvider.use", { defaultValue: "使用" })}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => void checkEntry(entry)}
                    disabled={testProvider.isPending}
                  >
                    {testProvider.isPending ? (
                      <Loader2 className="mr-1 h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Activity className="mr-1 h-3.5 w-3.5" />
                    )}
                    {t("share.networkProvider.check", {
                      defaultValue: "检测连通",
                    })}
                  </Button>
                  {onOpenShareSettings && (
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={onOpenShareSettings}
                    >
                      <BarChart3 className="mr-1 h-3.5 w-3.5" />
                      {t("share.networkProvider.usage", {
                        defaultValue: "查看用量",
                      })}
                    </Button>
                  )}
                </div>
                {lastCheck?.target === entry.targetId && (
                  <span
                    className={`hidden max-w-48 truncate text-xs md:block ${lastCheck.success ? "text-emerald-600" : "text-destructive"}`}
                    title={lastCheck.message}
                  >
                    {lastCheck.message}
                  </span>
                )}
              </div>
            );
          })
        )}
        {(!isProxyRunning || !isProxyTakeover || !status.bridgeRunning) && (
          <p className="rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-900 dark:text-amber-200">
            {t("share.networkProvider.proxyRequired", {
              app: getAppLabel(appId),
              defaultValue:
                "请先开启 {{app}} 的本地路由接管，共享网络请求才会生效。",
            })}
          </p>
        )}
      </div>
    </section>
  );
}
