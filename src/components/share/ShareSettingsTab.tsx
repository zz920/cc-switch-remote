import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Ban,
  KeyRound,
  Loader2,
  LogOut,
  RotateCcw,
  Save,
  ShieldAlert,
  TriangleAlert,
} from "lucide-react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { copyText } from "@/lib/clipboard";
import { extractErrorMessage } from "@/utils/errorUtils";
import { getAppLabel } from "@/config/appConfig";
import type { AppId } from "@/lib/api";
import { useProvidersQuery } from "@/lib/query/queries";
import {
  useBlockPeer,
  useLeaveNetwork,
  useRegenerateKey,
  useSetNodeName,
  useSetQuota,
  useSetRelayAddr,
  useSetSharedProviders,
  useShareKeyStorage,
  useShareStatus,
  useUnblockPeer,
} from "@/lib/query/share";
import type { SharePeer, ShareStatus } from "@/types/share";

/** 支持出借供应商的应用 */
const SHARE_APP_IDS: AppId[] = [
  "claude",
  "codex",
  "gemini",
  "grokbuild",
  "opencode",
  "openclaw",
];

export function ShareSettingsTab() {
  const { t } = useTranslation();
  const { data: status, isLoading } = useShareStatus();
  const { data: keyStorage } = useShareKeyStorage();

  if (isLoading) {
    return (
      <div className="flex items-center justify-center py-12">
        <Loader2 className="h-6 w-6 animate-spin text-muted-foreground" />
      </div>
    );
  }

  return (
    <div className="space-y-6">
      {keyStorage === "file" && (
        <Alert variant="destructive">
          <ShieldAlert className="h-4 w-4" />
          <AlertTitle>{t("share.settings.keyFileWarningTitle")}</AlertTitle>
          <AlertDescription>
            {t("share.settings.keyFileWarning")}
          </AlertDescription>
        </Alert>
      )}

      {!status?.joined && (
        <div className="rounded-xl border border-border bg-muted/40 p-4">
          <p className="text-sm text-muted-foreground">
            {t("share.settings.notJoinedHint")}
          </p>
        </div>
      )}

      <NodeNameSection status={status} />
      <RelaySection status={status} />
      <SharedProvidersSection status={status} />
      <QuotaSection status={status} />
      <PeerManagementSection status={status} />
      <DangerZoneSection status={status} />
    </div>
  );
}

// ========== 通用区块容器 ==========

interface SectionCardProps {
  title: string;
  description?: string;
  children: ReactNode;
}

function SectionCard({ title, description, children }: SectionCardProps) {
  return (
    <section className="space-y-4 rounded-xl border border-border bg-muted/40 p-4">
      <div>
        <h4 className="text-sm font-semibold">{title}</h4>
        {description && (
          <p className="text-xs text-muted-foreground">{description}</p>
        )}
      </div>
      {children}
    </section>
  );
}

// ========== 节点显示名 ==========

function NodeNameSection({ status }: { status?: ShareStatus }) {
  const { t } = useTranslation();
  const setNodeName = useSetNodeName();
  const [name, setName] = useState(status?.nodeName ?? "");

  useEffect(() => {
    if (status) setName(status.nodeName);
  }, [status]);

  const handleBlur = async () => {
    const trimmed = name.trim();
    if (!status || trimmed === status.nodeName) return;
    try {
      await setNodeName.mutateAsync(trimmed);
      toast.success(t("share.toast.saved"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <SectionCard
      title={t("share.settings.nodeName.title")}
      description={t("share.settings.nodeName.description")}
    >
      <Input
        value={name}
        onChange={(event) => setName(event.target.value)}
        onBlur={() => void handleBlur()}
        placeholder={t("share.settings.nodeName.placeholder")}
        maxLength={64}
        disabled={!status}
      />
    </SectionCard>
  );
}

// ========== Relay 地址 ==========

function RelaySection({ status }: { status?: ShareStatus }) {
  const { t } = useTranslation();
  const setRelayAddr = useSetRelayAddr();
  const [addr, setAddr] = useState(status?.relayAddr ?? "");

  useEffect(() => {
    if (status) setAddr(status.relayAddr ?? "");
  }, [status]);

  const handleSave = async (value: string | null) => {
    try {
      await setRelayAddr.mutateAsync(value);
      toast.success(t("share.toast.saved"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <SectionCard
      title={t("share.settings.relay.title")}
      description={t("share.settings.relay.description")}
    >
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <Input
          value={addr}
          onChange={(event) => setAddr(event.target.value)}
          placeholder={t("share.settings.relay.placeholder")}
          disabled={!status}
          className="flex-1"
        />
        <div className="flex gap-2">
          <Button
            size="sm"
            disabled={!status || setRelayAddr.isPending}
            onClick={() => void handleSave(addr.trim() || null)}
          >
            {setRelayAddr.isPending ? (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            ) : (
              <Save className="mr-2 h-4 w-4" />
            )}
            {t("common.save")}
          </Button>
          <Button
            size="sm"
            variant="outline"
            disabled={!status || setRelayAddr.isPending}
            onClick={() => {
              setAddr("");
              void handleSave(null);
            }}
          >
            <RotateCcw className="mr-2 h-4 w-4" />
            {t("share.settings.relay.resetDefault")}
          </Button>
        </div>
      </div>
    </SectionCard>
  );
}

// ========== 我共享的供应商 ==========

function SharedProvidersSection({ status }: { status?: ShareStatus }) {
  const { t } = useTranslation();
  const setSharedProviders = useSetSharedProviders();
  const [selected, setSelected] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (status) setSelected(new Set(status.sharedProviderIds));
  }, [status]);

  const toggle = (entry: string, checked: boolean) => {
    setSelected((previous) => {
      const next = new Set(previous);
      if (checked) {
        next.add(entry);
      } else {
        next.delete(entry);
      }
      return next;
    });
  };

  const handleSave = async () => {
    try {
      await setSharedProviders.mutateAsync(Array.from(selected));
      toast.success(t("share.toast.saved"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <SectionCard
      title={t("share.settings.sharedProviders.title")}
      description={t("share.settings.sharedProviders.description")}
    >
      <div className="space-y-4">
        {SHARE_APP_IDS.map((appId) => (
          <SharedProviderGroup
            key={appId}
            appId={appId}
            selected={selected}
            disabled={!status}
            onToggle={toggle}
          />
        ))}
      </div>
      <div className="flex justify-end">
        <Button
          size="sm"
          disabled={!status || setSharedProviders.isPending}
          onClick={() => void handleSave()}
        >
          {setSharedProviders.isPending ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Save className="mr-2 h-4 w-4" />
          )}
          {t("common.save")}
        </Button>
      </div>
    </SectionCard>
  );
}

interface SharedProviderGroupProps {
  appId: AppId;
  selected: Set<string>;
  disabled: boolean;
  onToggle: (entry: string, checked: boolean) => void;
}

function SharedProviderGroup({
  appId,
  selected,
  disabled,
  onToggle,
}: SharedProviderGroupProps) {
  const { t } = useTranslation();
  const { data } = useProvidersQuery(appId);
  const providers = useMemo(() => Object.values(data?.providers ?? {}), [data]);

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <span className="text-xs font-semibold text-foreground/80">
          {getAppLabel(appId)}
        </span>
        <div className="h-px flex-1 bg-border/50" />
      </div>
      {providers.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          {t("share.settings.sharedProviders.empty")}
        </p>
      ) : (
        <div className="grid gap-2 sm:grid-cols-2">
          {providers.map((provider) => {
            const entry = `${appId}:${provider.id}`;
            const isOfficial = provider.category === "official";
            return (
              <label
                key={provider.id}
                className={`flex items-center gap-2 rounded-md border border-border bg-background/60 px-3 py-2 text-sm ${
                  isOfficial || disabled
                    ? "cursor-not-allowed opacity-60"
                    : "cursor-pointer hover:bg-muted/50"
                }`}
                title={
                  isOfficial
                    ? t("share.settings.sharedProviders.officialNotAllowed")
                    : undefined
                }
              >
                <Checkbox
                  checked={selected.has(entry)}
                  disabled={isOfficial || disabled}
                  onCheckedChange={(checked) => onToggle(entry, checked)}
                />
                <span className="min-w-0 flex-1 truncate">{provider.name}</span>
                {isOfficial && (
                  <span className="flex-shrink-0 text-xs text-muted-foreground">
                    {t("share.settings.sharedProviders.officialBadge")}
                  </span>
                )}
              </label>
            );
          })}
        </div>
      )}
    </div>
  );
}

// ========== 用量限额 ==========

function QuotaSection({ status }: { status?: ShareStatus }) {
  const { t } = useTranslation();
  const setQuota = useSetQuota();
  const [scope, setScope] = useState<string>(status?.quotaScope ?? "daily");
  const [maxTokens, setMaxTokens] = useState(
    String(status?.quotaMaxTokens ?? 0),
  );
  const [perPeer, setPerPeer] = useState(status?.quotaPerPeer ?? false);

  useEffect(() => {
    if (status) {
      setScope(status.quotaScope === "monthly" ? "monthly" : "daily");
      setMaxTokens(String(status.quotaMaxTokens ?? 0));
      setPerPeer(status.quotaPerPeer);
    }
  }, [status]);

  const handleSave = async () => {
    const parsed = Number.parseInt(maxTokens, 10);
    if (Number.isNaN(parsed) || parsed < 0) {
      toast.error(t("share.quota.invalidMaxTokens"));
      return;
    }
    try {
      await setQuota.mutateAsync({ scope, maxTokens: parsed, perPeer });
      toast.success(t("share.toast.saved"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  const peersWithUsage = status?.peers ?? [];

  return (
    <SectionCard
      title={t("share.quota.title")}
      description={t("share.quota.description")}
    >
      <div className="grid gap-4 md:grid-cols-2">
        <div className="space-y-2">
          <Label>{t("share.quota.scope")}</Label>
          <Select value={scope} onValueChange={setScope} disabled={!status}>
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="daily">{t("share.quota.daily")}</SelectItem>
              <SelectItem value="monthly">
                {t("share.quota.monthly")}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="space-y-2">
          <Label>{t("share.quota.maxTokens")}</Label>
          <Input
            type="number"
            min={0}
            value={maxTokens}
            onChange={(event) => setMaxTokens(event.target.value)}
            placeholder={t("share.quota.maxTokensPlaceholder")}
            disabled={!status}
          />
          <p className="text-xs text-muted-foreground">
            {t("share.quota.maxTokensHint")}
          </p>
        </div>
      </div>

      <div className="flex items-center justify-between rounded-md border border-border bg-background/60 px-3 py-2">
        <Label className="text-sm font-medium">
          {t("share.quota.perPeer")}
        </Label>
        <Switch
          checked={perPeer}
          onCheckedChange={setPerPeer}
          disabled={!status}
        />
      </div>

      <div className="flex justify-end">
        <Button
          size="sm"
          disabled={!status || setQuota.isPending}
          onClick={() => void handleSave()}
        >
          {setQuota.isPending ? (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          ) : (
            <Save className="mr-2 h-4 w-4" />
          )}
          {t("common.save")}
        </Button>
      </div>

      {peersWithUsage.length > 0 && (
        <div className="space-y-2 border-t border-border/50 pt-3">
          <p className="text-xs font-medium text-muted-foreground">
            {t("share.quota.usageTitle")}
          </p>
          <div className="space-y-2">
            {peersWithUsage.map((peer) => (
              <PeerUsageRow key={peer.peerId} peer={peer} />
            ))}
          </div>
        </div>
      )}
    </SectionCard>
  );
}

function PeerUsageRow({ peer }: { peer: SharePeer }) {
  const { t } = useTranslation();
  const used = peer.tokensUsed;
  const total = peer.quotaRemaining != null ? used + peer.quotaRemaining : null;
  const percent =
    total && total > 0 ? Math.min(100, (used / total) * 100) : null;

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between text-xs">
        <span className="font-medium">{peer.name}</span>
        <span className="text-muted-foreground">
          {total != null
            ? t("share.quota.usageSummary", {
                used: used.toLocaleString(),
                total: total.toLocaleString(),
              })
            : t("share.quota.usageUnlimited", {
                used: used.toLocaleString(),
              })}
        </span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          className={`h-full rounded-full transition-all ${
            percent != null && percent >= 90 ? "bg-destructive" : "bg-primary"
          }`}
          style={{ width: `${percent ?? 0}%` }}
        />
      </div>
    </div>
  );
}

// ========== 节点管理 ==========

function PeerManagementSection({ status }: { status?: ShareStatus }) {
  const { t } = useTranslation();
  const blockPeer = useBlockPeer();
  const unblockPeer = useUnblockPeer();
  const [blockTarget, setBlockTarget] = useState<SharePeer | null>(null);

  const peers = status?.peers ?? [];
  const busy = blockPeer.isPending || unblockPeer.isPending;

  const handleBlock = async () => {
    if (!blockTarget) return;
    try {
      await blockPeer.mutateAsync(blockTarget.peerId);
      toast.success(t("share.toast.peerBlocked"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    } finally {
      setBlockTarget(null);
    }
  };

  const handleUnblock = async (peerId: string) => {
    try {
      await unblockPeer.mutateAsync(peerId);
      toast.success(t("share.toast.peerUnblocked"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <SectionCard
      title={t("share.peers.managementTitle")}
      description={t("share.peers.managementDescription")}
    >
      {peers.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          {t("share.peers.empty")}
        </p>
      ) : (
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t("share.peers.name")}</TableHead>
              <TableHead>{t("share.approval.peerId")}</TableHead>
              <TableHead>{t("share.peers.statusColumn")}</TableHead>
              <TableHead>{t("share.peers.connectionColumn")}</TableHead>
              <TableHead>{t("share.peers.used")}</TableHead>
              <TableHead className="text-right">
                {t("common.actions")}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {peers.map((peer) => (
              <TableRow key={peer.peerId}>
                <TableCell className="font-medium">
                  <span className="mr-2">{peer.name}</span>
                  {peer.isBlocked && (
                    <Badge variant="destructive" className="text-xs">
                      {t("share.peers.blocked")}
                    </Badge>
                  )}
                </TableCell>
                <TableCell className="font-mono text-xs">
                  {peer.peerId.slice(0, 8)}
                </TableCell>
                <TableCell>
                  <span
                    className={`inline-flex items-center gap-1.5 text-xs ${
                      peer.online
                        ? "text-emerald-600 dark:text-emerald-400"
                        : "text-muted-foreground"
                    }`}
                  >
                    <span
                      className={`h-1.5 w-1.5 rounded-full ${
                        peer.online
                          ? "bg-emerald-500"
                          : "bg-muted-foreground/40"
                      }`}
                    />
                    {peer.online
                      ? t("share.peers.online")
                      : t("share.peers.offline")}
                  </span>
                </TableCell>
                <TableCell className="text-xs">
                  {peer.direct
                    ? t("share.peers.direct")
                    : t("share.peers.relay")}
                </TableCell>
                <TableCell className="text-xs">
                  {peer.tokensUsed.toLocaleString()}
                </TableCell>
                <TableCell className="text-right">
                  {peer.isBlocked ? (
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy}
                      onClick={() => void handleUnblock(peer.peerId)}
                    >
                      {t("share.peers.unblock")}
                    </Button>
                  ) : (
                    <Button
                      size="sm"
                      variant="destructive"
                      disabled={busy}
                      onClick={() => setBlockTarget(peer)}
                    >
                      <Ban className="mr-1.5 h-3.5 w-3.5" />
                      {t("share.peers.block")}
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}

      <ConfirmDialog
        isOpen={blockTarget != null}
        variant="destructive"
        title={t("share.peers.blockConfirmTitle")}
        message={t("share.peers.blockConfirmMessage", {
          name: blockTarget?.name ?? "",
        })}
        confirmText={t("share.peers.block")}
        pending={blockPeer.isPending}
        onConfirm={() => void handleBlock()}
        onCancel={() => setBlockTarget(null)}
      />
    </SectionCard>
  );
}

// ========== 危险区 ==========

function DangerZoneSection({ status }: { status?: ShareStatus }) {
  const { t } = useTranslation();
  const regenerateKey = useRegenerateKey();
  const leaveNetwork = useLeaveNetwork();
  const [regenerateConfirmOpen, setRegenerateConfirmOpen] = useState(false);
  const [leaveConfirmOpen, setLeaveConfirmOpen] = useState(false);
  const [newKey, setNewKey] = useState<string | null>(null);

  const handleRegenerate = async () => {
    try {
      const key = await regenerateKey.mutateAsync();
      setNewKey(key);
      toast.success(t("share.toast.keyRegenerated"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    } finally {
      setRegenerateConfirmOpen(false);
    }
  };

  const handleLeave = async () => {
    try {
      await leaveNetwork.mutateAsync();
      toast.success(t("share.toast.left"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    } finally {
      setLeaveConfirmOpen(false);
    }
  };

  const handleCopyKey = async () => {
    if (!newKey) return;
    try {
      await copyText(newKey);
      toast.success(t("share.toast.copied"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <section className="space-y-4 rounded-xl border border-destructive/40 bg-destructive/5 p-4">
      <div>
        <h4 className="flex items-center gap-2 text-sm font-semibold text-destructive">
          <TriangleAlert className="h-4 w-4" />
          {t("share.dangerZone.title")}
        </h4>
        <p className="text-xs text-muted-foreground">
          {t("share.dangerZone.description")}
        </p>
      </div>

      <div className="flex flex-wrap gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={!status?.joined}
          onClick={() => setRegenerateConfirmOpen(true)}
        >
          <KeyRound className="mr-2 h-4 w-4" />
          {t("share.dangerZone.regenerateKey")}
        </Button>
        <Button
          size="sm"
          variant="destructive"
          disabled={!status?.joined}
          onClick={() => setLeaveConfirmOpen(true)}
        >
          <LogOut className="mr-2 h-4 w-4" />
          {status?.role === "creator"
            ? t("share.dangerZone.dissolve")
            : t("share.dangerZone.leave")}
        </Button>
      </div>

      <ConfirmDialog
        isOpen={regenerateConfirmOpen}
        variant="destructive"
        title={t("share.dangerZone.regenerateConfirmTitle")}
        message={t("share.dangerZone.regenerateConfirmMessage")}
        confirmText={t("share.dangerZone.regenerateKey")}
        pending={regenerateKey.isPending}
        onConfirm={() => void handleRegenerate()}
        onCancel={() => setRegenerateConfirmOpen(false)}
      />

      <ConfirmDialog
        isOpen={leaveConfirmOpen}
        variant="destructive"
        title={
          status?.role === "creator"
            ? t("share.dangerZone.dissolveConfirmTitle")
            : t("share.leave.confirmTitle")
        }
        message={
          status?.role === "creator"
            ? t("share.dangerZone.dissolveConfirmMessage")
            : t("share.leave.confirmMessage")
        }
        confirmText={
          status?.role === "creator"
            ? t("share.dangerZone.dissolve")
            : t("share.dangerZone.leave")
        }
        pending={leaveNetwork.isPending}
        onConfirm={() => void handleLeave()}
        onCancel={() => setLeaveConfirmOpen(false)}
      />

      {/* 新密钥仅展示一次 */}
      <Dialog
        open={newKey != null}
        onOpenChange={(open) => {
          if (!open) setNewKey(null);
        }}
      >
        <DialogContent className="max-w-md" zIndex="alert">
          <DialogHeader>
            <DialogTitle>
              {t("share.dangerZone.regenerateSuccessTitle")}
            </DialogTitle>
          </DialogHeader>
          <div className="space-y-3 px-6">
            <code className="block break-all rounded border border-border/60 bg-background px-3 py-2 font-mono text-sm">
              {newKey}
            </code>
            <div className="flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3">
              <TriangleAlert className="mt-0.5 h-4 w-4 flex-shrink-0 text-amber-600 dark:text-amber-400" />
              <p className="text-xs text-amber-700 dark:text-amber-300">
                {t("share.create.shareKeyWarning")}
              </p>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => void handleCopyKey()}>
              {t("common.copy")}
            </Button>
            <Button onClick={() => setNewKey(null)}>{t("common.done")}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  );
}
