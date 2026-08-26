import {
  Activity,
  AlertTriangle,
  BarChart3,
  Check,
  Loader2,
  Network,
  RefreshCw,
  Route,
} from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { getAppLabel, isProxyAppId } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import {
  useSetRouteTargets,
  useSetRoutePreference,
  useShareStatus,
  useTestSharedProvider,
} from "@/lib/query/share";
import type { ShareProviderInfo } from "@/types/share";
import { extractErrorMessage } from "@/utils/errorUtils";

interface ShareProviderRouteCardProps {
  appId: AppId;
  isProxyRunning: boolean;
  isProxyTakeover: boolean;
  onOpenShareSettings?: () => void;
  onOpenRoutingSettings?: () => void;
}

interface ProviderGroup {
  providerId: string;
  name: string;
  models: string[];
  targets: Array<{ peerId: string; providerId: string }>;
}

/** 共享网络 Provider 卡片（与本地 Provider 分离，仅按节点能力动态展示）。 */
export function ShareProviderRouteCard({
  appId,
  isProxyRunning,
  isProxyTakeover,
  onOpenShareSettings,
  onOpenRoutingSettings,
}: ShareProviderRouteCardProps) {
  const { t } = useTranslation();
  const { data: status, refetch, isFetching } = useShareStatus();
  const setRouteTargets = useSetRouteTargets();
  const setRoutePreference = useSetRoutePreference();
  const testProvider = useTestSharedProvider();
  const [lastCheck, setLastCheck] = useState<
    { target: string; message: string; success: boolean } | undefined
  >();

  if (
    !isProxyAppId(appId) ||
    !status?.joined ||
    status.routePreference === "local_only"
  ) {
    return null;
  }

  const groups = new Map<string, ProviderGroup>();
  for (const peer of status.peers ?? []) {
    if (!peer.online || peer.isBlocked) continue;
    for (const provider of (peer.providers ?? []) as ShareProviderInfo[]) {
      if (provider.app !== appId) continue;
      const existing = groups.get(provider.providerId) ?? {
        providerId: provider.providerId,
        name: provider.name,
        models: [],
        targets: [],
      };
      existing.models = Array.from(
        new Set([...existing.models, ...(provider.models ?? [])]),
      ).slice(0, 8);
      existing.targets.push({
        peerId: peer.peerId,
        providerId: provider.providerId,
      });
      groups.set(provider.providerId, existing);
    }
  }
  const providerGroups = Array.from(groups.values());
  const allTargets = providerGroups.flatMap((group) =>
    group.targets.map((target) => `${target.peerId}:${target.providerId}`),
  );
  const configuredTargets = status.routeTargets?.[appId];
  const selectedTargets = new Set(
    configuredTargets === undefined ? allTargets : configuredTargets,
  );
  const routingReady =
    selectedTargets.size > 0 &&
    isProxyRunning &&
    isProxyTakeover &&
    status.bridgeRunning;

  const saveTargets = async (targets: string[]) => {
    try {
      await setRouteTargets.mutateAsync({ appType: appId, targets });
      toast.success(
        t("share.networkProvider.saved", {
          defaultValue: "共享 Provider 路由已更新",
        }),
        { closeButton: true },
      );
      return true;
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
      return false;
    }
  };

  const useGroup = async (group: ProviderGroup) => {
    const saved = await saveTargets(
      group.targets.map((target) => `${target.peerId}:${target.providerId}`),
    );
    // “使用”是明确切换到共享 Provider；本地优先模式下将共享网络调到前面。
    if (saved && status.routePreference === "local_first") {
      try {
        await setRoutePreference.mutateAsync("network_first");
      } catch (error) {
        toast.error(
          t("share.toast.failed", { detail: extractErrorMessage(error) }),
        );
      }
    }
  };

  const setGroupSelection = (group: ProviderGroup, checked: boolean) => {
    const next = new Set(selectedTargets);
    for (const target of group.targets) {
      const targetId = `${target.peerId}:${target.providerId}`;
      if (checked) next.add(targetId);
      else next.delete(targetId);
    }
    return saveTargets(Array.from(next));
  };

  const checkGroup = async (group: ProviderGroup) => {
    const target = group.targets[0];
    if (!target) return;
    try {
      const result = await testProvider.mutateAsync({
        appType: appId,
        peerId: target.peerId,
        providerId: target.providerId,
      });
      setLastCheck({
        target: group.providerId,
        message: result.message,
        success: result.success,
      });
      if (result.success) {
        toast.success(
          t("share.networkProvider.checkPassed", {
            defaultValue: "共享 Provider 连通性正常",
          }),
        );
      } else {
        toast.error(result.message);
      }
    } catch (error) {
      const message = extractErrorMessage(error);
      setLastCheck({ target: group.providerId, message, success: false });
      toast.error(message);
    }
  };

  return (
    <section className="overflow-hidden rounded-xl border border-blue-500/25 bg-blue-500/5 shadow-sm">
      <div className="flex flex-wrap items-start justify-between gap-3 border-b border-border/50 px-4 py-3">
        <div className="flex min-w-0 items-start gap-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-blue-500/10 ring-1 ring-blue-500/20">
            <Network className="h-5 w-5 text-blue-500" />
          </div>
          <div className="min-w-0">
            <h3 className="font-semibold">
              {t("share.networkProvider.title", {
                defaultValue: "共享网络 Provider",
              })}
            </h3>
            <p className="text-xs leading-relaxed text-muted-foreground">
              {t("share.networkProvider.description", {
                app: getAppLabel(appId),
                defaultValue:
                  "选择在线节点为 {{app}} 提供的远端 Provider；本地 Provider 不会被替换。",
              })}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Badge variant={routingReady ? "default" : "secondary"}>
            {routingReady
              ? t("share.networkProvider.active", { defaultValue: "正在使用" })
              : t("share.networkProvider.waiting", { defaultValue: "可配置" })}
          </Badge>
          <Button
            size="icon"
            variant="ghost"
            onClick={() => void refetch()}
            disabled={isFetching}
            aria-label={t("share.networkProvider.refresh", {
              defaultValue: "刷新共享 Provider",
            })}
          >
            <RefreshCw
              className={`h-4 w-4 ${isFetching ? "animate-spin" : ""}`}
            />
          </Button>
        </div>
      </div>

      <div className="space-y-3 p-4">
        {providerGroups.length === 0 ? (
          <p className="rounded-lg border border-dashed border-border px-3 py-4 text-sm text-muted-foreground">
            {t("share.networkProvider.noneAvailable", {
              app: getAppLabel(appId),
              defaultValue: "当前没有在线节点为 {{app}} 提供可用 Provider。",
            })}
          </p>
        ) : (
          providerGroups.map((group) => {
            const targetIds = group.targets.map(
              (target) => `${target.peerId}:${target.providerId}`,
            );
            const groupSelected = targetIds.filter((id) =>
              selectedTargets.has(id),
            ).length;
            const active = groupSelected > 0;
            return (
              <div
                key={group.providerId}
                className="rounded-lg border border-border bg-background/70 p-3"
              >
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="flex min-w-0 items-start gap-3">
                    <Checkbox
                      checked={groupSelected === targetIds.length}
                      onCheckedChange={(checked) =>
                        void setGroupSelection(group, checked === true)
                      }
                      aria-label={t("share.networkProvider.selectProvider", {
                        name: group.name,
                        defaultValue: "选择 {{name}}",
                      })}
                    />
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium">{group.name}</span>
                        <Badge variant="outline">
                          {t("share.networkProvider.providerCount", {
                            count: group.targets.length,
                            defaultValue: "{{count}} 个 Provider",
                          })}
                        </Badge>
                        {active && (
                          <Badge variant="secondary">
                            {t("share.networkProvider.selected", {
                              defaultValue: "已选择",
                            })}
                          </Badge>
                        )}
                      </div>
                      <div className="mt-1 flex flex-wrap gap-1">
                        {group.models.length > 0 ? (
                          group.models.map((model) => (
                            <Badge
                              key={model}
                              variant="outline"
                              className="text-[11px]"
                            >
                              {model}
                            </Badge>
                          ))
                        ) : (
                          <span className="text-xs text-muted-foreground">
                            {t("share.networkProvider.modelsUnknown", {
                              defaultValue: "模型信息待刷新",
                            })}
                          </span>
                        )}
                      </div>
                    </div>
                  </div>
                  <div className="flex flex-wrap items-center gap-1">
                    <Button
                      size="sm"
                      variant={active ? "default" : "outline"}
                      onClick={() => void useGroup(group)}
                      disabled={
                        setRouteTargets.isPending ||
                        setRoutePreference.isPending
                      }
                    >
                      {active && <Check className="mr-1.5 h-3.5 w-3.5" />}
                      {t("share.networkProvider.use", { defaultValue: "使用" })}
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => void checkGroup(group)}
                      disabled={testProvider.isPending}
                    >
                      {testProvider.isPending ? (
                        <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
                      ) : (
                        <Activity className="mr-1.5 h-3.5 w-3.5" />
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
                        <BarChart3 className="mr-1.5 h-3.5 w-3.5" />
                        {t("share.networkProvider.usage", {
                          defaultValue: "查看用量",
                        })}
                      </Button>
                    )}
                  </div>
                </div>
                {lastCheck?.target === group.providerId && (
                  <p
                    className={`mt-2 text-xs ${lastCheck.success ? "text-emerald-600" : "text-destructive"}`}
                  >
                    {lastCheck.message}
                  </p>
                )}
              </div>
            );
          })
        )}

        {!isProxyRunning || !isProxyTakeover || !status.bridgeRunning ? (
          <div
            role="alert"
            className="flex flex-col gap-3 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-3 text-sm text-amber-900 dark:text-amber-200 sm:flex-row sm:items-center sm:justify-between"
          >
            <div className="flex items-start gap-2">
              <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" />
              <p>
                {t("share.networkProvider.proxyRequired", {
                  app: getAppLabel(appId),
                  defaultValue:
                    "共享 Provider 已选择，但 {{app}} 需要先开启本地路由接管，请求才会经过共享网络。",
                })}
              </p>
            </div>
            {onOpenRoutingSettings && (
              <Button
                size="sm"
                variant="outline"
                className="shrink-0"
                onClick={onOpenRoutingSettings}
              >
                <Route className="mr-2 h-4 w-4" />
                {t("share.networkProvider.openRouting", {
                  defaultValue: "配置路由",
                })}
              </Button>
            )}
          </div>
        ) : null}
      </div>
    </section>
  );
}
