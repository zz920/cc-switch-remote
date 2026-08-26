import { useEffect, useMemo, useState } from "react";
import {
  Check,
  ChevronDown,
  Copy,
  Loader2,
  LogOut,
  Network,
  Settings2,
  Share2,
  ShieldAlert,
  TriangleAlert,
} from "lucide-react";
import { toast } from "sonner";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { copyText } from "@/lib/clipboard";
import { extractErrorMessage } from "@/utils/errorUtils";
import { getAppLabel } from "@/config/appConfig";
import {
  useApproveJoin,
  useCancelJoin,
  useCreateNetwork,
  useLeaveNetwork,
  useRejectJoin,
  useRequestJoin,
  useSetRoutePreference,
  useShareStatus,
} from "@/lib/query/share";
import type {
  CreateNetworkResult,
  JoinRequest,
  ShareRoutePreference,
  ShareStatus,
} from "@/types/share";

/** 从用户输入中提取 share id（支持粘贴完整 tokentap://join?id= 链接） */
export function extractShareId(input: string): string {
  const trimmed = input.trim();
  const match = trimmed.match(/tokentap:\/\/join\?id=([^\s&]+)/i);
  if (match) return match[1];
  return trimmed;
}

const ROUTE_PREFERENCES: ShareRoutePreference[] = [
  "local_first",
  "network_first",
  "network_only",
];

function formatCountdown(remainingSeconds: number): string {
  const total = Math.max(0, remainingSeconds);
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

interface ShareNetworkSectionProps {
  showApprovals?: boolean;
  onOpenSettings: () => void;
}

export function ShareNetworkSection({
  showApprovals = true,
  onOpenSettings,
}: ShareNetworkSectionProps) {
  const { t } = useTranslation();
  const { data: status, isLoading } = useShareStatus();
  const [open, setOpen] = useState(true);
  const [createDialogOpen, setCreateDialogOpen] = useState(false);
  const [joinDialogOpen, setJoinDialogOpen] = useState(false);
  const [leaveConfirmOpen, setLeaveConfirmOpen] = useState(false);

  const leaveNetwork = useLeaveNetwork();

  const incomingCount = status?.incomingRequests.length ?? 0;
  const onlineCount = useMemo(
    () => status?.peers.filter((peer) => peer.online).length ?? 0,
    [status?.peers],
  );

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

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <div className="rounded-xl border border-border bg-card/50 transition-colors">
        <CollapsibleTrigger asChild>
          <button
            type="button"
            className="flex w-full items-center justify-between gap-3 p-4 text-left hover:bg-muted/50 rounded-xl"
          >
            <div className="flex items-center gap-3">
              <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-background ring-1 ring-border">
                <Share2 className="h-4 w-4 text-blue-500" />
              </div>
              <div className="space-y-1">
                <p className="text-sm font-medium leading-none">
                  {t("share.title")}
                </p>
                <p className="text-xs text-muted-foreground">
                  {status?.joined
                    ? t("share.status.connectedSummary", {
                        id: status.shareId,
                        count: onlineCount,
                      })
                    : status?.pendingJoin
                      ? t("share.status.pending")
                      : t("share.status.notJoined")}
                </p>
              </div>
            </div>
            <div className="flex items-center gap-2">
              {incomingCount > 0 && (
                <Badge variant="destructive" className="gap-1">
                  {t("share.approval.badge", { count: incomingCount })}
                </Badge>
              )}
              {status?.joined && (
                <Badge variant="default" className="gap-1.5">
                  <span className="h-1.5 w-1.5 rounded-full bg-emerald-300" />
                  {t("share.status.connected")}
                </Badge>
              )}
              <ChevronDown
                className={`h-4 w-4 text-muted-foreground transition-transform ${
                  open ? "rotate-180" : ""
                }`}
              />
            </div>
          </button>
        </CollapsibleTrigger>

        <CollapsibleContent>
          <div className="space-y-4 border-t border-border/50 px-4 pb-4 pt-4">
            {isLoading ? (
              <div className="flex items-center justify-center py-6">
                <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
              </div>
            ) : !status?.joined && status?.pendingJoin ? (
              <PendingJoinView
                shortCode={status.pendingJoin.shortCode}
                expiresAt={status.pendingJoin.expiresAt}
              />
            ) : !status?.joined ? (
              <div className="space-y-3">
                <p className="text-sm text-muted-foreground">
                  {t("share.intro")}
                </p>
                <div className="flex flex-wrap gap-2">
                  <Button size="sm" onClick={() => setJoinDialogOpen(true)}>
                    {t("share.join.button")}
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    onClick={() => setCreateDialogOpen(true)}
                  >
                    {t("share.create.button")}
                  </Button>
                </div>
              </div>
            ) : (
              <JoinedView
                status={status}
                onlineCount={onlineCount}
                onOpenSettings={onOpenSettings}
                onLeave={() => setLeaveConfirmOpen(true)}
              />
            )}

            {showApprovals && incomingCount > 0 && (
              <div className="space-y-2">
                {status?.incomingRequests.map((request) => (
                  <ApprovalCard key={request.peerId} request={request} />
                ))}
              </div>
            )}
          </div>
        </CollapsibleContent>
      </div>

      <CreateNetworkDialog
        open={createDialogOpen}
        onOpenChange={setCreateDialogOpen}
      />
      <JoinNetworkDialog
        open={joinDialogOpen}
        onOpenChange={setJoinDialogOpen}
      />
      <ConfirmDialog
        isOpen={leaveConfirmOpen}
        variant="destructive"
        title={t("share.leave.confirmTitle")}
        message={t("share.leave.confirmMessage")}
        confirmText={t("share.leave.button")}
        pending={leaveNetwork.isPending}
        onConfirm={() => void handleLeave()}
        onCancel={() => setLeaveConfirmOpen(false)}
      />
    </Collapsible>
  );
}

// ========== 已加入视图 ==========

interface JoinedViewProps {
  status: ShareStatus;
  onlineCount: number;
  onOpenSettings: () => void;
  onLeave: () => void;
}

function JoinedView({
  status,
  onlineCount,
  onOpenSettings,
  onLeave,
}: JoinedViewProps) {
  const { t } = useTranslation();
  const setRoutePreference = useSetRoutePreference();

  const handleRouteChange = async (preference: ShareRoutePreference) => {
    if (preference === status.routePreference) return;
    try {
      await setRoutePreference.mutateAsync(preference);
      toast.success(t("share.toast.saved"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <div className="space-y-4">
      {/* 状态行 */}
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm">
        <span className="inline-flex items-center gap-1.5 text-emerald-600 dark:text-emerald-400">
          <span className="h-2 w-2 rounded-full bg-emerald-500" />
          {t("share.status.connected")}
        </span>
        <span className="font-mono text-muted-foreground">
          {status.shareId}
        </span>
        <span className="text-muted-foreground">·</span>
        <span className="text-muted-foreground">
          {t("share.status.peersOnline", { count: onlineCount })}
        </span>
        <span className="text-muted-foreground">·</span>
        <Badge variant="secondary">
          {status.role === "creator"
            ? t("share.status.roleCreator")
            : t("share.status.roleMember")}
        </Badge>
      </div>

      {/* 节点列表简表 */}
      <div className="space-y-2">
        <p className="text-xs font-medium text-muted-foreground">
          {t("share.peers.title")}
        </p>
        {status.peers.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            {t("share.peers.empty")}
          </p>
        ) : (
          <div className="space-y-1.5">
            {status.peers.map((peer) => (
              <div
                key={peer.peerId}
                className="flex items-center justify-between gap-2 rounded-md border border-border bg-background/60 px-3 py-2 text-sm"
              >
                <div className="flex min-w-0 items-center gap-2">
                  <span
                    className={`h-1.5 w-1.5 flex-shrink-0 rounded-full ${
                      peer.online ? "bg-emerald-500" : "bg-muted-foreground/40"
                    }`}
                  />
                  <span
                    className="truncate font-medium"
                    title={peer.name || undefined}
                  >
                    {peer.name ||
                      t("share.peers.unnamed", {
                        defaultValue: "未命名节点",
                      })}
                  </span>
                  <span className="flex-shrink-0 text-xs text-muted-foreground">
                    {peer.online
                      ? peer.direct
                        ? t("share.peers.direct")
                        : t("share.peers.relay")
                      : t("share.peers.offline")}
                  </span>
                </div>
                <div className="flex flex-shrink-0 flex-wrap justify-end gap-1">
                  {peer.sharedApps.map((app) => (
                    <Badge key={app} variant="outline" className="text-xs">
                      {getAppLabel(app)}
                    </Badge>
                  ))}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>

      {/* 路由偏好 */}
      <div className="space-y-2">
        <p className="text-xs font-medium text-muted-foreground">
          {t("share.routePreference.title")}
        </p>
        <div
          role="radiogroup"
          aria-label={t("share.routePreference.title")}
          className="flex flex-wrap gap-2"
        >
          {ROUTE_PREFERENCES.map((preference) => {
            const active = status.routePreference === preference;
            return (
              <button
                key={preference}
                type="button"
                role="radio"
                aria-checked={active}
                disabled={setRoutePreference.isPending}
                onClick={() => void handleRouteChange(preference)}
                className={`inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm transition-colors ${
                  active
                    ? "border-primary/40 bg-primary/10 text-primary font-medium"
                    : "border-border bg-background/60 hover:bg-muted/50"
                }`}
              >
                {active && <Check className="h-3.5 w-3.5" />}
                {t(`share.routePreference.${preference}`)}
              </button>
            );
          })}
        </div>
      </div>

      {/* 操作 */}
      <div className="flex flex-wrap gap-2 border-t border-border/50 pt-3">
        <Button size="sm" variant="outline" onClick={onOpenSettings}>
          <Settings2 className="mr-2 h-4 w-4" />
          {t("share.peers.manage")}
        </Button>
        <Button size="sm" variant="destructive" onClick={onLeave}>
          <LogOut className="mr-2 h-4 w-4" />
          {t("share.leave.button")}
        </Button>
      </div>
    </div>
  );
}

// ========== 等待审批视图 ==========

interface PendingJoinViewProps {
  shortCode: string;
  expiresAt: number;
}

function PendingJoinView({ shortCode, expiresAt }: PendingJoinViewProps) {
  const { t } = useTranslation();
  const cancelJoin = useCancelJoin();
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));

  useEffect(() => {
    const timer = window.setInterval(() => {
      setNow(Math.floor(Date.now() / 1000));
    }, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const remaining = expiresAt - now;

  const handleCancel = async () => {
    try {
      await cancelJoin.mutateAsync();
      toast.success(t("share.toast.joinCancelled"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <div className="space-y-3 rounded-lg border border-amber-500/30 bg-amber-500/5 p-4">
      <div className="space-y-1">
        <p className="text-sm font-medium">{t("share.join.pendingTitle")}</p>
        <p className="text-xs text-muted-foreground">
          {t("share.join.pendingHint")}
        </p>
      </div>
      <p className="text-center font-mono text-3xl font-semibold tracking-[0.3em]">
        {shortCode}
      </p>
      <div className="flex items-center justify-between gap-2">
        <p className="text-xs text-muted-foreground">
          {remaining > 0
            ? t("share.join.expiresIn", {
                time: formatCountdown(remaining),
              })
            : t("share.join.expired")}
        </p>
        <Button
          size="sm"
          variant="outline"
          disabled={cancelJoin.isPending}
          onClick={() => void handleCancel()}
        >
          {cancelJoin.isPending && (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          )}
          {t("share.join.cancel")}
        </Button>
      </div>
    </div>
  );
}

// ========== 审批卡片 ==========

export function ApprovalCard({ request }: { request: JoinRequest }) {
  const { t } = useTranslation();
  const approveJoin = useApproveJoin();
  const rejectJoin = useRejectJoin();

  const handleApprove = async () => {
    try {
      await approveJoin.mutateAsync(request.peerId);
      toast.success(t("share.toast.approved"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  const handleReject = async () => {
    try {
      await rejectJoin.mutateAsync(request.peerId);
      toast.success(t("share.toast.rejected"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  const busy = approveJoin.isPending || rejectJoin.isPending;

  return (
    <div className="space-y-3 rounded-lg border border-primary/30 bg-primary/5 p-4">
      <div className="flex items-center gap-2">
        <ShieldAlert className="h-4 w-4 text-primary" />
        <p className="text-sm font-medium">{t("share.approval.title")}</p>
      </div>
      <div className="grid gap-1 text-sm">
        <p className="font-medium">{request.nodeName}</p>
        <div className="flex min-w-0 items-start gap-2 text-xs text-muted-foreground">
          <span className="shrink-0">{t("share.approval.peerId")}:</span>
          <span className="min-w-0 break-all font-mono" title={request.peerId}>
            {request.peerId}
          </span>
          <Button
            size="icon"
            variant="ghost"
            className="h-6 w-6 shrink-0"
            title={t("share.approval.copyPeerId", {
              defaultValue: "复制 PeerId",
            })}
            onClick={() => void copyText(request.peerId)}
          >
            <Copy className="h-3.5 w-3.5" />
          </Button>
        </div>
        <p className="text-xs text-muted-foreground">
          {t("share.approval.shortCode")}:{" "}
          <span className="font-mono text-base font-semibold tracking-[0.2em] text-foreground">
            {request.shortCode}
          </span>
        </p>
        <p className="text-xs text-amber-600 dark:text-amber-400">
          {t("share.approval.verifyHint")}
        </p>
        <p className="text-xs text-muted-foreground">
          {t("share.approval.receivedAt", { defaultValue: "收到时间" })}:{" "}
          {new Date(request.receivedAt * 1000).toLocaleString()}
        </p>
      </div>
      <div className="flex gap-2">
        <Button size="sm" disabled={busy} onClick={() => void handleApprove()}>
          {approveJoin.isPending && (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          )}
          {t("share.approval.approve")}
        </Button>
        <Button
          size="sm"
          variant="outline"
          disabled={busy}
          onClick={() => void handleReject()}
        >
          {rejectJoin.isPending && (
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
          )}
          {t("share.approval.reject")}
        </Button>
      </div>
    </div>
  );
}

// ========== 创建网络对话框 ==========

interface CreateNetworkDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function CreateNetworkDialog({ open, onOpenChange }: CreateNetworkDialogProps) {
  const { t } = useTranslation();
  const createNetwork = useCreateNetwork();
  const [nodeName, setNodeName] = useState("");
  const [result, setResult] = useState<CreateNetworkResult | null>(null);

  useEffect(() => {
    if (open) {
      setNodeName("");
      setResult(null);
    }
  }, [open]);

  const handleCreate = async () => {
    try {
      const created = await createNetwork.mutateAsync(
        nodeName.trim() || undefined,
      );
      setResult(created);
      toast.success(t("share.toast.created"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  const shareLink = result ? `tokentap://join?id=${result.shareId}` : "";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" zIndex="nested">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Network className="h-5 w-5 text-blue-500" />
            {result
              ? t("share.create.successTitle")
              : t("share.create.dialogTitle")}
          </DialogTitle>
          {!result && (
            <DialogDescription>
              {t("share.create.dialogDescription")}
            </DialogDescription>
          )}
        </DialogHeader>

        {result ? (
          <div className="space-y-4 px-6">
            <CopyableField
              label={t("share.create.shareIdLabel")}
              value={result.shareId}
            />
            <div className="space-y-2">
              <CopyableField
                label={t("share.create.shareKeyLabel")}
                value={result.shareKey}
              />
              <div className="flex items-start gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-3">
                <TriangleAlert className="mt-0.5 h-4 w-4 flex-shrink-0 text-amber-600 dark:text-amber-400" />
                <p className="text-xs text-amber-700 dark:text-amber-300">
                  {t("share.create.shareKeyWarning")}
                </p>
              </div>
            </div>
            <div className="space-y-1">
              <CopyableField
                label={t("share.create.shareLinkLabel")}
                value={shareLink}
              />
              <p className="text-xs text-muted-foreground">
                {t("share.create.shareLinkHint")}
              </p>
            </div>
          </div>
        ) : (
          <div className="space-y-2 px-6">
            <Label htmlFor="share-node-name">
              {t("share.create.nodeNameLabel")}
            </Label>
            <Input
              id="share-node-name"
              value={nodeName}
              onChange={(event) => setNodeName(event.target.value)}
              placeholder={t("share.create.nodeNamePlaceholder")}
              maxLength={64}
            />
            <p className="text-xs text-muted-foreground">
              {t("share.create.nodeNameDescription")}
            </p>
          </div>
        )}

        <DialogFooter>
          {result ? (
            <Button onClick={() => onOpenChange(false)}>
              {t("common.done", { defaultValue: "完成" })}
            </Button>
          ) : (
            <>
              <Button variant="ghost" onClick={() => onOpenChange(false)}>
                {t("common.cancel")}
              </Button>
              <Button
                disabled={createNetwork.isPending}
                onClick={() => void handleCreate()}
              >
                {createNetwork.isPending && (
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                )}
                {t("share.create.submit")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ========== 加入网络对话框 ==========

interface JoinNetworkDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function JoinNetworkDialog({ open, onOpenChange }: JoinNetworkDialogProps) {
  const { t } = useTranslation();
  const requestJoin = useRequestJoin();
  const [input, setInput] = useState("");

  useEffect(() => {
    if (open) setInput("");
  }, [open]);

  const handleJoin = async () => {
    const shareId = extractShareId(input);
    if (!shareId) {
      toast.error(t("share.join.invalidShareId"));
      return;
    }
    try {
      await requestJoin.mutateAsync(shareId);
      toast.success(t("share.toast.joinRequested"), { closeButton: true });
      onOpenChange(false);
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md" zIndex="nested">
        <DialogHeader>
          <DialogTitle>{t("share.join.dialogTitle")}</DialogTitle>
          <DialogDescription>
            {t("share.join.dialogDescription")}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-2 px-6">
          <Label htmlFor="share-join-id">{t("share.join.shareIdLabel")}</Label>
          <Input
            id="share-join-id"
            value={input}
            onChange={(event) => setInput(event.target.value)}
            placeholder={t("share.join.shareIdPlaceholder")}
            onKeyDown={(event) => {
              if (event.key === "Enter") void handleJoin();
            }}
          />
          <p className="text-xs text-muted-foreground">
            {t("share.join.shareIdDescription")}
          </p>
        </div>
        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button
            disabled={requestJoin.isPending || !input.trim()}
            onClick={() => void handleJoin()}
          >
            {requestJoin.isPending && (
              <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            )}
            {t("share.join.submit")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

// ========== 通用：可复制字段 ==========

interface CopyableFieldProps {
  label: string;
  value: string;
}

function CopyableField({ label, value }: CopyableFieldProps) {
  const { t } = useTranslation();

  const handleCopy = async () => {
    try {
      await copyText(value);
      toast.success(t("share.toast.copied"), { closeButton: true });
    } catch (error) {
      toast.error(
        t("share.toast.failed", { detail: extractErrorMessage(error) }),
      );
    }
  };

  return (
    <div className="space-y-1">
      <p className="text-xs text-muted-foreground">{label}</p>
      <div className="flex items-center gap-2">
        <code className="flex-1 break-all rounded border border-border/60 bg-background px-3 py-2 font-mono text-sm">
          {value}
        </code>
        <Button size="sm" variant="outline" onClick={() => void handleCopy()}>
          <Copy className="mr-1.5 h-3.5 w-3.5" />
          {t("common.copy")}
        </Button>
      </div>
    </div>
  );
}
