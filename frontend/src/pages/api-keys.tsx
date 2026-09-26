import { useContext, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Copy, Check, Key, Edit, Globe, Layers, Settings2, ArrowRightLeft, SearchX, X } from "lucide-react";
import { BadgeOverflowList } from "@/components/BadgeOverflowList";
import { GroupsBadge } from "@/components/GroupsBadge";
import { GroupMultiSelect } from "@/components/groups/GroupPicker";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Switch } from "@/components/ui/switch";
import { Checkbox } from "@/components/ui/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Virtuoso } from "react-virtuoso";
import {
  useApiKeys,
  createApiKeyOptimistic,
  updateApiKeyOptimistic,
  deleteApiKeyOptimistic,
  batchDeleteApiKeysOptimistic,
  useDashboardGroups,
  useTransformRegistry,
} from "@/lib/swr";
import type { ApiKey, ApiKeyCreated, CreateApiKeyInput, Group, ModelRedirectRule, RequestCaptureMode, RequestCaptureRetention, TransformRuleConfig, UpdateApiKeyInput } from "@/lib/api";
import { api as apiClient } from "@/lib/api";
import { AnimatedButton, PageWrapper, motion } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { DataTableShell, TableToolbarSearch } from "@/components/ui/data-table-shell";
import {
  DataList,
  DataListActions,
  DataListCell,
  DataListHead,
  DataListHeader,
} from "@/components/ui/data-list";
import { virtualDataListComponents } from "@/components/ui/data-list-virtual";
import { StatusBadge } from "@/components/ui/status";
import { EmptyState } from "@/components/ui/empty-state";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { findFirstInvalidTransformRule } from "@/components/transforms/transform-schema";
import { toast } from "sonner";
import { useAuth } from "@/hooks/use-auth";
import { normalizeMultiplier } from "@/lib/exact-decimal";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import { cn } from "@/lib/utils";

function parseOptionalMultiplier(value: string): string | undefined {
  if (!value.trim()) return undefined;
  const normalized = normalizeMultiplier(value);
  if (normalized == null) {
    throw new Error("Multiplier must be a non-negative decimal with at most 9 fractional digits");
  }
  return normalized;
}

function parseOptionalNanoBalance(value: string, allowNegative = true): string | undefined {
  const trimmed = value.trim();
  if (!trimmed) return undefined;
  if (!/^-?(?:0|[1-9]\d*)$/.test(trimmed)) {
    throw new Error("Balance must be a signed integer nano-USD string");
  }
  const parsed = BigInt(trimmed);
  const minimum = -(1n << 127n);
  const maximum = (1n << 127n) - 1n;
  if (parsed < minimum || parsed > maximum) {
    throw new Error("Balance exceeds the supported signed 128-bit range");
  }
  if (!allowNegative && parsed < 0n) {
    throw new Error("Initial balance must be non-negative");
  }
  return parsed.toString();
}

function requestCaptureBadgeVariant(mode: RequestCaptureMode): "secondary" | "outline" {
  return mode === "capture-only-abnormal" ? "secondary" : "outline";
}

function ApiKeyRestrictionBadges({
  apiKey,
  t,
}: {
  apiKey: ApiKey;
  t: (key: string) => string;
}) {
  const captureLabel =
    apiKey.request_capture_mode === "capture-only-abnormal"
      ? t("apiKeys.captureBadgeAbnormal")
      : t("apiKeys.captureBadgeAll");
  const captureHelp =
    apiKey.request_capture_mode === "capture-only-abnormal"
      ? t("apiKeys.requestCaptureModeAbnormalHelp")
      : t("apiKeys.requestCaptureModeAllHelp");
  const items = [
    ...(apiKey.model_limits_enabled && apiKey.model_limits.length > 0
      ? [
          {
            key: "model-limits",
            collapsed: (
              <Badge variant="outline" className="gap-1 px-1.5 text-xs">
                <Layers className="h-3 w-3 shrink-0" />
                {apiKey.model_limits.length}
              </Badge>
            ),
            full: (
              <Badge variant="outline" className="max-w-none gap-1 px-1.5 text-xs">
                <Layers className="h-3 w-3 shrink-0" />
                <span className="whitespace-nowrap">
                  {t("apiKeys.modelLimits")}: {apiKey.model_limits.join(", ")}
                </span>
              </Badge>
            ),
          },
        ]
      : []),
    ...(apiKey.ip_whitelist.length > 0
      ? [
          {
            key: "ip-whitelist",
            collapsed: (
              <Badge variant="outline" className="gap-1 px-1.5 text-xs">
                <Globe className="h-3 w-3 shrink-0" />
                {apiKey.ip_whitelist.length}
              </Badge>
            ),
            full: (
              <Badge variant="outline" className="max-w-none gap-1 px-1.5 text-xs">
                <Globe className="h-3 w-3 shrink-0" />
                <span className="whitespace-nowrap">
                  {t("apiKeys.ipWhitelist")}: {apiKey.ip_whitelist.join(", ")}
                </span>
              </Badge>
            ),
          },
        ]
      : []),
    ...(apiKey.max_multiplier != null
      ? [
          {
            key: "max-multiplier",
            collapsed: (
              <Badge variant="outline" className="px-1.5 text-xs">
                ≤{apiKey.max_multiplier}x
              </Badge>
            ),
            full: (
              <Badge variant="outline" className="max-w-none px-1.5 text-xs">
                <span className="whitespace-nowrap">
                  {t("apiKeys.maxMultiplier")}: {apiKey.max_multiplier}x
                </span>
              </Badge>
            ),
          },
        ]
      : []),
    ...(apiKey.request_capture_mode !== "off"
      ? [
          {
            key: "request-capture",
            collapsed: (
              <Badge
                variant={requestCaptureBadgeVariant(apiKey.request_capture_mode)}
                className="px-1.5 text-xs"
              >
                {captureLabel}
              </Badge>
            ),
            full: (
              <Badge
                variant={requestCaptureBadgeVariant(apiKey.request_capture_mode)}
                className="max-w-none px-1.5 text-xs"
              >
                <span className="whitespace-nowrap">
                  {captureLabel}: {captureHelp}
                </span>
              </Badge>
            ),
          },
        ]
      : []),
  ];

  if (items.length === 0) {
    return <span className="text-muted-foreground">—</span>;
  }

  return (
    <BadgeOverflowList
      items={items}
      visibleCount={2}
      popoverOnSingle
      ariaLabel={t("apiKeys.restrictions")}
      className="max-w-[24rem]"
      contentClassName="max-w-[min(34rem,calc(100vw-2rem))]"
    />
  );
}

/** The key text shown in the list; search matches exactly this text (AK-UI-SEARCH-1). */
function displayedKeyPrefix(key: ApiKey): string {
  return key.key ? key.key.slice(0, 12) : key.key_prefix;
}

/** AK-UI-SEARCH-1 match rule; filtering and selection pruning must agree on it. */
function keyMatchesQuery(key: ApiKey, needle: string): boolean {
  return (
    needle === ""
    || key.name.toLowerCase().includes(needle)
    || displayedKeyPrefix(key).toLowerCase().includes(needle)
  );
}

interface KeyGroupsSectionProps {
  idPrefix: string;
  useUserGroup: boolean;
  groupIds: string[];
  groups: Group[];
  groupsLoading: boolean;
  ownerGroupId: string | null;
  isAdmin: boolean;
  onUseUserGroupChange: (next: boolean) => void;
  onGroupIdsChange: (next: string[]) => void;
}

/**
 * TM-GRP-1..TM-GRP-5: a key either inherits the owner's single group or holds
 * an ordered explicit selection; non-admins may only pick `user_selectable`
 * groups plus their own current group.
 */
function KeyGroupsSection({
  idPrefix,
  useUserGroup,
  groupIds,
  groups,
  groupsLoading,
  ownerGroupId,
  isAdmin,
  onUseUserGroupChange,
  onGroupIdsChange,
}: KeyGroupsSectionProps) {
  const { t } = useTranslation();
  const ownerGroup = useMemo(
    () => groups.find((group) => group.id === ownerGroupId) ?? null,
    [groups, ownerGroupId]
  );

  return (
    <div className="space-y-3 rounded-lg border p-3">
      <div className="flex items-center justify-between gap-3">
        <div>
          <Label htmlFor={`${idPrefix}-use-user-group`}>{t("apiKeys.useUserGroup")}</Label>
          <p className="mt-1 text-xs text-muted-foreground">
            {ownerGroup
              ? t("apiKeys.useUserGroupHelpNamed", { name: ownerGroup.name })
              : t("apiKeys.useUserGroupHelp")}
          </p>
        </div>
        <Switch
          id={`${idPrefix}-use-user-group`}
          checked={useUserGroup}
          onCheckedChange={onUseUserGroupChange}
        />
      </div>
      {!useUserGroup && (
        <div className="space-y-2">
          <Label>{t("apiKeys.groups")}</Label>
          <GroupMultiSelect
            value={groupIds}
            groups={groups}
            loading={groupsLoading}
            sortable
            optionFilter={
              isAdmin
                ? undefined
                : (group) => group.user_selectable || group.id === ownerGroupId
            }
            onChange={onGroupIdsChange}
          />
          <p className="text-xs text-muted-foreground">{t("apiKeys.groupsHelp")}</p>
        </div>
      )}
    </div>
  );
}

interface ModelRedirectsEditorProps {
  value: ModelRedirectRule[];
  onChange: (next: ModelRedirectRule[]) => void;
}

function ModelRedirectsEditor({ value, onChange }: ModelRedirectsEditorProps) {
  const { t } = useTranslation();

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <ArrowRightLeft className="h-4 w-4 text-muted-foreground" />
          <h3 className="text-sm font-medium">{t("apiKeys.modelRedirects")}</h3>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => onChange([...value, { pattern: "", replace: "" }])}
        >
          <Plus className="mr-1 h-3 w-3" />
          {t("common.add")}
        </Button>
      </div>
      {value.length === 0 && (
        <p className="text-sm text-muted-foreground">{t("apiKeys.modelRedirectsEmpty")}</p>
      )}
      {value.map((rule, idx) => (
        <div key={idx} className="flex items-center gap-2">
          <Input
            value={rule.pattern}
            onChange={(e) => {
              const updated = [...value];
              updated[idx] = { ...updated[idx], pattern: e.target.value };
              onChange(updated);
            }}
            placeholder=".*opus.*"
            className="flex-1 font-mono text-sm"
          />
          <span className="shrink-0 text-sm text-muted-foreground">→</span>
          <Input
            value={rule.replace}
            onChange={(e) => {
              const updated = [...value];
              updated[idx] = { ...updated[idx], replace: e.target.value };
              onChange(updated);
            }}
            placeholder="gpt-5.4"
            className="flex-1 font-mono text-sm"
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="size-11 touch-manipulation sm:size-8 shrink-0"
            aria-label={t("common.delete")}
            onClick={() => onChange(value.filter((_, i) => i !== idx))}
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <p className="text-xs text-muted-foreground">{t("apiKeys.modelRedirectsHelp")}</p>
    </div>
  );
}

export function ApiKeysPage() {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const { data: keys = [], isLoading } = useApiKeys();
  const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups();
  const canManageSystem = currentUser?.role === "admin" || currentUser?.role === "super_admin";
  // /transforms/registry is admin-only; skip it for non-admins to avoid a 403 loop.
  const { data: transformRegistry = [], isLoading: transformRegistryLoading } =
    useTransformRegistry({ isPaused: () => !canManageSystem });
  const apiKeyTransformRegistry = useMemo(
    () => transformRegistry.filter((item) => item.supported_scopes.includes("api_key")),
    [transformRegistry]
  );
  const [createOpen, setCreateOpen] = useState(false);
  const [editKey, setEditKey] = useState<ApiKey | null>(null);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const [batchDeleteOpen, setBatchDeleteOpen] = useState(false);
  const [query, setQuery] = useState("");
  const scrollParent = useContext(DashboardScrollParentContext);
  const needle = query.trim().toLowerCase();
  const visibleKeys = useMemo(
    () => keys.filter((key) => keyMatchesQuery(key, needle)),
    [keys, needle]
  );

  // Create form state
  const [newKeyName, setNewKeyName] = useState("");
  const [newKeyExpires, setNewKeyExpires] = useState("");
  const [newKeySubAccountEnabled, setNewKeySubAccountEnabled] = useState(false);
  const [newKeySubAccountBalanceNanoUsd, setNewKeySubAccountBalanceNanoUsd] = useState("0");
  const [transferDialogKey, setTransferDialogKey] = useState<ApiKey | null>(null);
  const [transferAmount, setTransferAmount] = useState("");
  const [transferring, setTransferring] = useState(false);
  const [newKeyModelLimitsEnabled, setNewKeyModelLimitsEnabled] = useState(false);
  const [newKeyModelLimits, setNewKeyModelLimits] = useState("");
  const [newKeyIpWhitelist, setNewKeyIpWhitelist] = useState("");

  const [newKeyUseUserGroup, setNewKeyUseUserGroup] = useState(true);
  const [newKeyGroupIds, setNewKeyGroupIds] = useState<string[]>([]);
  const [newKeyMaxMultiplier, setNewKeyMaxMultiplier] = useState("");
  const [newKeyTransforms, setNewKeyTransforms] = useState<TransformRuleConfig[]>([]);
  const [newKeyModelRedirects, setNewKeyModelRedirects] = useState<ModelRedirectRule[]>([]);
  const [newKeyReasoningEnvelopeEnabled, setNewKeyReasoningEnvelopeEnabled] = useState(true);
  const [newKeyRequestCaptureMode, setNewKeyRequestCaptureMode] = useState<RequestCaptureMode>("off");
  const [newKeyRequestCaptureRetention, setNewKeyRequestCaptureRetention] = useState<RequestCaptureRetention>("24h");

  const [creating, setCreating] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [createdKey, setCreatedKey] = useState<ApiKeyCreated | null>(null);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const resetCreateForm = () => {
    setNewKeyName("");
    setNewKeyExpires("");
    setNewKeySubAccountEnabled(false);
    setNewKeySubAccountBalanceNanoUsd("0");
    setNewKeyModelLimitsEnabled(false);
    setNewKeyModelLimits("");
    setNewKeyIpWhitelist("");

    setNewKeyUseUserGroup(true);
    setNewKeyGroupIds([]);
    setNewKeyMaxMultiplier("");
    setNewKeyTransforms([]);
    setNewKeyModelRedirects([]);
    setNewKeyReasoningEnvelopeEnabled(true);
    setNewKeyRequestCaptureMode("off");
    setNewKeyRequestCaptureRetention("24h");
  };

  const handleCreate = async () => {
    if (!newKeyName.trim()) return;
    const invalidRule = findFirstInvalidTransformRule(newKeyTransforms, apiKeyTransformRegistry);
    if (invalidRule) {
      const firstError = invalidRule.errors[0];
      toast.error(t("transforms.validationRuleInvalid", {
        index: invalidRule.index + 1,
        reason: `${firstError.field} ${firstError.message}`,
      }));
      return;
    }
    if (!newKeyUseUserGroup && newKeyGroupIds.length === 0) {
      toast.error(t("apiKeys.groupsRequired"));
      return;
    }
    setCreating(true);
    try {
      const initialSubAccountBalance = canManageSystem
        ? parseOptionalNanoBalance(newKeySubAccountBalanceNanoUsd, false)
        : undefined;
      if (
        !newKeySubAccountEnabled
        && initialSubAccountBalance != null
        && BigInt(initialSubAccountBalance) !== 0n
      ) {
        throw new Error("A non-zero initial balance requires sub-account billing to be enabled");
      }
      const input: CreateApiKeyInput = {
        name: newKeyName.trim(),
        expires_in_days: newKeyExpires ? parseInt(newKeyExpires) : undefined,
        sub_account_enabled: newKeySubAccountEnabled,
        ...(canManageSystem
          ? { sub_account_balance_nano_usd: initialSubAccountBalance }
          : {}),
        model_limits_enabled: newKeyModelLimitsEnabled,
        model_limits: newKeyModelLimits ? newKeyModelLimits.split(",").map(s => s.trim()).filter(s => s) : [],
        ip_whitelist: newKeyIpWhitelist ? newKeyIpWhitelist.split(",").map(s => s.trim()).filter(s => s) : [],
        use_user_group: newKeyUseUserGroup,
        group_ids: newKeyUseUserGroup ? [] : newKeyGroupIds,
        max_multiplier: parseOptionalMultiplier(newKeyMaxMultiplier),
        transforms: newKeyTransforms,
        model_redirects: newKeyModelRedirects.filter((r) => r.pattern.trim() && r.replace.trim()),
        reasoning_envelope_enabled: newKeyReasoningEnvelopeEnabled,
        request_capture_mode: newKeyRequestCaptureMode,
        request_capture_retention: newKeyRequestCaptureRetention,
      };
      const key = await createApiKeyOptimistic(
        input,
        keys
      );
      setCreatedKey(key);
      resetCreateForm();
      setCreateOpen(false);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedCreate"));
    } finally {
      setCreating(false);
    }
  };

  const handleUpdate = async () => {
    if (!editKey) return;
    const invalidRule = findFirstInvalidTransformRule(newKeyTransforms, apiKeyTransformRegistry);
    if (invalidRule) {
      const firstError = invalidRule.errors[0];
      toast.error(t("transforms.validationRuleInvalid", {
        index: invalidRule.index + 1,
        reason: `${firstError.field} ${firstError.message}`,
      }));
      return;
    }
    if (!newKeyUseUserGroup && newKeyGroupIds.length === 0) {
      toast.error(t("apiKeys.groupsRequired"));
      return;
    }
    setUpdating(true);
    try {
      const input: UpdateApiKeyInput = {
        name: newKeyName.trim() || undefined,
        sub_account_enabled: newKeySubAccountEnabled,
        ...(canManageSystem && newKeySubAccountEnabled
          ? { sub_account_balance_nano_usd: parseOptionalNanoBalance(newKeySubAccountBalanceNanoUsd) }
          : {}),
        model_limits_enabled: newKeyModelLimitsEnabled,
        model_limits: newKeyModelLimits ? newKeyModelLimits.split(",").map(s => s.trim()).filter(s => s) : [],
        ip_whitelist: newKeyIpWhitelist ? newKeyIpWhitelist.split(",").map(s => s.trim()).filter(s => s) : [],
        use_user_group: newKeyUseUserGroup,
        group_ids: newKeyUseUserGroup ? [] : newKeyGroupIds,
        max_multiplier: parseOptionalMultiplier(newKeyMaxMultiplier),
        transforms: newKeyTransforms,
        model_redirects: newKeyModelRedirects.filter((r) => r.pattern.trim() && r.replace.trim()),
        reasoning_envelope_enabled: newKeyReasoningEnvelopeEnabled,
        request_capture_mode: newKeyRequestCaptureMode,
        request_capture_retention: newKeyRequestCaptureRetention,
      };
      await updateApiKeyOptimistic(
        editKey.id,
        input,
        keys
      );
      setEditKey(null);
      resetCreateForm();
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedUpdate"));
    } finally {
      setUpdating(false);
    }
  };

  const handleDelete = async (id: string) => {
    setDeleteTargetId(id);
  };

  const confirmDelete = async () => {
    if (!deleteTargetId) return;
    try {
      await deleteApiKeyOptimistic(
        deleteTargetId,
        keys
      );
      setSelectedKeys(prev => prev.filter(k => k !== deleteTargetId));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedDelete"));
    } finally {
      setDeleteTargetId(null);
    }
  };

  const handleBatchDelete = async () => {
    if (selectedKeys.length === 0) return;
    setBatchDeleteOpen(true);
  };

  const confirmBatchDelete = async () => {
    try {
      await batchDeleteApiKeysOptimistic(
        selectedKeys,
        keys
      );
      setSelectedKeys([]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedBatchDelete"));
    } finally {
      setBatchDeleteOpen(false);
    }
  };

  const handleToggleEnabled = async (key: ApiKey) => {
    try {
      await updateApiKeyOptimistic(
        key.id,
        { enabled: !key.enabled },
        keys
      );
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedUpdate"));
    }
  };

  const handleCopy = async (key: string) => {
    await navigator.clipboard.writeText(key);
    setCopiedKey(key);
    setTimeout(() => setCopiedKey(null), 2000);
  };

  const openEditDialog = (key: ApiKey) => {
    setEditKey(key);
    setNewKeyName(key.name);
    setNewKeySubAccountEnabled(key.sub_account_enabled);
    setNewKeySubAccountBalanceNanoUsd(key.sub_account_balance_nano_usd);
    setNewKeyModelLimitsEnabled(key.model_limits_enabled);
    setNewKeyModelLimits(key.model_limits.join(", "));
    setNewKeyIpWhitelist(key.ip_whitelist.join(", "));
    setNewKeyUseUserGroup(key.use_user_group);
    setNewKeyGroupIds(key.group_ids ?? []);
    setNewKeyMaxMultiplier(key.max_multiplier != null ? String(key.max_multiplier) : "");
    setNewKeyTransforms(key.transforms ?? []);
    setNewKeyModelRedirects(key.model_redirects ?? []);
    setNewKeyReasoningEnvelopeEnabled(key.reasoning_envelope_enabled ?? true);
    setNewKeyRequestCaptureMode(key.request_capture_mode ?? "off");
    setNewKeyRequestCaptureRetention(key.request_capture_retention ?? "24h");
  };

  const toggleSelectKey = (id: string) => {
    setSelectedKeys(prev =>
      prev.includes(id) ? prev.filter(k => k !== id) : [...prev, id]
    );
  };

  const allVisibleSelected =
    visibleKeys.length > 0 && visibleKeys.every((key) => selectedKeys.includes(key.id));

  const toggleSelectAll = () => {
    setSelectedKeys(allVisibleSelected ? [] : visibleKeys.map((key) => key.id));
  };

  const handleQueryChange = (value: string) => {
    setQuery(value);
    const nextNeedle = value.trim().toLowerCase();
    setSelectedKeys((prev) =>
      prev.filter((id) => keys.some((key) => key.id === id && keyMatchesQuery(key, nextNeedle)))
    );
  };

  const formatDate = (date: string) => {
    return new Date(date).toLocaleDateString(undefined, {
      year: "numeric",
      month: "short",
      day: "numeric",
    });
  };

  if (isLoading) {
    return (
      <PageWrapper className="space-y-6">
        <TablePageSkeleton />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <PageHeader title={t("apiKeys.title")} description={t("apiKeys.description")} actions={(
        <>
        {selectedKeys.length > 0 && (
          <Button variant="destructive" onClick={handleBatchDelete}>
            <Trash2 className="mr-2 h-4 w-4" />
            {t("apiKeys.batchDelete")}
          </Button>
        )}
        <Dialog open={createOpen} onOpenChange={setCreateOpen}>
          <DialogTrigger asChild>
            <AnimatedButton>
              <Button>
                <Plus className="mr-2 h-4 w-4" />
                {t("apiKeys.createKey")}
              </Button>
            </AnimatedButton>
          </DialogTrigger>
          <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-4xl">
            <div className="flex min-h-0 flex-col p-6">
            <DialogHeader className="shrink-0">
              <DialogTitle>{t("apiKeys.createApiKey")}</DialogTitle>
              <DialogDescription>
                {t("apiKeys.createDescription")}
              </DialogDescription>
            </DialogHeader>
            <div className="min-h-0 flex-1 space-y-4 overflow-y-auto px-1 py-4 -mx-1">
              <div className="space-y-2">
                <Label htmlFor="name">{t("common.name")}</Label>
                <Input
                  id="name"
                  value={newKeyName}
                  onChange={(e) => setNewKeyName(e.target.value)}
                  placeholder="My API Key"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="expires">{t("apiKeys.expiresInDays")}</Label>
                <Input
                  id="expires"
                  type="number"
                  min="1"
                  value={newKeyExpires}
                  onChange={(e) => setNewKeyExpires(e.target.value)}
                  placeholder="30"
                />
              </div>
              <KeyGroupsSection
                idPrefix="create"
                useUserGroup={newKeyUseUserGroup}
                groupIds={newKeyGroupIds}
                groups={groups}
                groupsLoading={groupsLoading}
                ownerGroupId={currentUser?.group_id ?? null}
                isAdmin={canManageSystem}
                onUseUserGroupChange={setNewKeyUseUserGroup}
                onGroupIdsChange={setNewKeyGroupIds}
              />
              <div className="flex items-center space-x-2">
                <Switch
                  id="subAccountEnabled"
                  checked={newKeySubAccountEnabled}
                  onCheckedChange={setNewKeySubAccountEnabled}
                />
                <Label htmlFor="subAccountEnabled">{t("apiKeys.subAccountEnabled")}</Label>
              </div>
              {canManageSystem && (
                <div className="space-y-2">
                  <Label htmlFor="subAccountBalanceNanoUsd">
                    {t("apiKeys.balance")} (nano-USD)
                  </Label>
                  <Input
                    id="subAccountBalanceNanoUsd"
                    type="text"
                    inputMode="numeric"
                    value={newKeySubAccountBalanceNanoUsd}
                    onChange={(event) => setNewKeySubAccountBalanceNanoUsd(event.target.value)}
                  />
                </div>
              )}
              <div className="space-y-1">
                <div className="flex items-center space-x-2">
                  <Switch
                    id="reasoningEnvelopeEnabled"
                    checked={newKeyReasoningEnvelopeEnabled}
                    onCheckedChange={setNewKeyReasoningEnvelopeEnabled}
                  />
                  <Label htmlFor="reasoningEnvelopeEnabled">{t("apiKeys.reasoningEnvelopeEnabled")}</Label>
                </div>
                <p className="text-sm text-muted-foreground">{t("apiKeys.reasoningEnvelopeHelp")}</p>
              </div>
              <div className="space-y-1">
                <div className="flex items-center space-x-2">
                  <Label htmlFor="requestCaptureMode">{t("apiKeys.requestCaptureMode")}</Label>
                </div>
                <Select value={newKeyRequestCaptureMode} onValueChange={(value) => setNewKeyRequestCaptureMode(value as RequestCaptureMode)}>
                  <SelectTrigger id="requestCaptureMode">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="off">{t("apiKeys.requestCaptureModeOff")}</SelectItem>
                    <SelectItem value="capture-all">{t("apiKeys.requestCaptureModeAll")}</SelectItem>
                    <SelectItem value="capture-only-abnormal">{t("apiKeys.requestCaptureModeAbnormal")}</SelectItem>
                  </SelectContent>
                </Select>
                <p className="text-sm text-muted-foreground">{t("apiKeys.requestCaptureHelp")}</p>
              </div>
              {newKeyRequestCaptureMode !== "off" && (
                <div className="space-y-1">
                  <Label htmlFor="requestCaptureRetention">{t("apiKeys.requestCaptureRetention")}</Label>
                  <Select value={newKeyRequestCaptureRetention} onValueChange={(value) => setNewKeyRequestCaptureRetention(value as RequestCaptureRetention)}>
                    <SelectTrigger id="requestCaptureRetention">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="5m">{t("apiKeys.requestCaptureRetention5m")}</SelectItem>
                      <SelectItem value="1h">{t("apiKeys.requestCaptureRetention1h")}</SelectItem>
                      <SelectItem value="24h">{t("apiKeys.requestCaptureRetention24h")}</SelectItem>
                      <SelectItem value="7d">{t("apiKeys.requestCaptureRetention7d")}</SelectItem>
                    </SelectContent>
                  </Select>
                  <p className="text-sm text-muted-foreground">{t("apiKeys.requestCaptureRetentionHelp")}</p>
                </div>
              )}
              <div className="flex items-center space-x-2">
                <Switch
                  id="modelLimitsEnabled"
                  checked={newKeyModelLimitsEnabled}
                  onCheckedChange={setNewKeyModelLimitsEnabled}
                />
                <Label htmlFor="modelLimitsEnabled">{t("apiKeys.enableModelLimits")}</Label>
              </div>
              {newKeyModelLimitsEnabled && (
                <div className="space-y-2">
                  <Label htmlFor="modelLimits">{t("apiKeys.allowedModels")}</Label>
                  <Input
                    id="modelLimits"
                    value={newKeyModelLimits}
                    onChange={(e) => setNewKeyModelLimits(e.target.value)}
                    placeholder="gpt-4, gpt-3.5-turbo"
                  />
                  <p className="text-sm text-muted-foreground">{t("apiKeys.modelsHelp")}</p>
                </div>
              )}
              <div className="space-y-2">
                <Label htmlFor="ipWhitelist">{t("apiKeys.ipWhitelist")}</Label>
                <Input
                  id="ipWhitelist"
                  value={newKeyIpWhitelist}
                  onChange={(e) => setNewKeyIpWhitelist(e.target.value)}
                  placeholder="192.168.1.1, 10.0.0.0/8"
                />
                <p className="text-sm text-muted-foreground">{t("apiKeys.ipHelp")}</p>
              </div>
              <div className="space-y-2">
                <Label htmlFor="maxMultiplier">{t("apiKeys.maxMultiplier")}</Label>
                <Input
                  id="maxMultiplier"
                  type="text"
                  inputMode="decimal"
                  value={newKeyMaxMultiplier}
                  onChange={(e) => setNewKeyMaxMultiplier(e.target.value)}
                  placeholder="e.g. 1.5"
                />
                <p className="text-sm text-muted-foreground">{t("apiKeys.maxMultiplierHelp")}</p>
              </div>
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Settings2 className="h-4 w-4 text-muted-foreground" />
                  <h3 className="text-sm font-medium">{t("transforms.titleApiKey")}</h3>
                </div>
                <TransformChainEditor
                  value={newKeyTransforms}
                  registry={apiKeyTransformRegistry}
                  loading={transformRegistryLoading}
                  onChange={setNewKeyTransforms}
                />
              </div>
              <ModelRedirectsEditor
                value={newKeyModelRedirects}
                onChange={setNewKeyModelRedirects}
              />
            </div>
            <DialogFooter className="shrink-0 pt-4">
              <Button variant="outline" onClick={() => { setCreateOpen(false); resetCreateForm(); }}>
                {t("common.cancel")}
              </Button>
              <Button onClick={handleCreate} disabled={creating || !newKeyName.trim()}>
                {creating ? t("common.creating") : t("common.create")}
              </Button>
            </DialogFooter>
            </div>
          </DialogContent>
        </Dialog>
        </>
      )} />

      {createdKey && (
        <motion.div
          initial={{ opacity: 0, y: 20, scale: 0.95 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ type: "spring", stiffness: 300, damping: 25 }}
        >
          <Card className="border-success-border bg-success-soft">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-success-foreground">
                <motion.div
                  initial={{ scale: 0 }}
                  animate={{ scale: 1 }}
                  transition={{ delay: 0.2, type: "spring", stiffness: 300 }}
                >
                  <Key className="h-5 w-5" />
                </motion.div>
                {t("apiKeys.apiKeyCreated")}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex items-center gap-2">
                <code className="flex-1 rounded-lg border bg-muted px-3 py-2 text-sm">
                  {createdKey.key}
                </code>
                <AnimatedButton>
                  <Button
                    variant="outline"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    aria-label={t("common.copy")}
                    onClick={() => handleCopy(createdKey.key)}
                  >
                    {copiedKey === createdKey.key ? (
                      <Check className="h-4 w-4" />
                    ) : (
                      <Copy className="h-4 w-4" />
                    )}
                  </Button>
                </AnimatedButton>
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="mt-2"
                onClick={() => setCreatedKey(null)}
              >
                {t("common.dismiss")}
              </Button>
            </CardContent>
          </Card>
        </motion.div>
      )}

      <DataTableShell
        toolbar={keys.length > 0 ? (
          <>
            <div className="flex w-full flex-wrap items-center gap-3 sm:w-auto">
              <div className="flex h-9 items-center gap-2">
                <Checkbox
                  id="api-keys-select-all"
                  checked={allVisibleSelected ? true : selectedKeys.length > 0 ? "indeterminate" : false}
                  disabled={visibleKeys.length === 0}
                  onCheckedChange={toggleSelectAll}
                />
                <Label htmlFor="api-keys-select-all" className="font-normal">
                  {t("apiKeys.selectAll")}
                </Label>
              </div>
              <TableToolbarSearch
                value={query}
                onChange={(event) => handleQueryChange(event.target.value)}
                placeholder={t("apiKeys.searchPlaceholder")}
                aria-label={t("apiKeys.searchPlaceholder")}
                containerClassName="min-w-0 flex-1 sm:w-64 sm:flex-none"
              />
            </div>
            <p className="text-sm tabular-nums text-muted-foreground">
              {t("apiKeys.keyCount", { count: visibleKeys.length })}
            </p>
          </>
        ) : undefined}
        isEmpty={keys.length === 0}
        emptyState={(
          <EmptyState
            icon={<Key className="size-10" aria-hidden="true" />}
            title={t("apiKeys.noKeysTitle")}
            description={t("apiKeys.noKeysDescription")}
            action={(
              <Button onClick={() => setCreateOpen(true)}>
                <Plus aria-hidden="true" />
                {t("apiKeys.createKey")}
              </Button>
            )}
          />
        )}
      >
        {visibleKeys.length === 0 ? (
          <EmptyState
            variant="inline"
            icon={<SearchX className="size-8" aria-hidden="true" />}
            title={t("apiKeys.noMatchesTitle")}
            description={t("apiKeys.noMatchesDescription", { query: query.trim() })}
          />
        ) : (
          <DataList columns="minmax(0,1.5fr) 10.5rem minmax(0,1fr) 9rem 4rem 7.5rem">
            <DataListHeader>
              <DataListHead className="pl-7">{t("common.name")}</DataListHead>
              <DataListHead align="end">{t("apiKeys.balance")}</DataListHead>
              <DataListHead>{t("apiKeys.restrictions")}</DataListHead>
              <DataListHead>{t("apiKeys.expires")}</DataListHead>
              <DataListHead>{t("common.status")}</DataListHead>
              <DataListHead align="end">{t("common.actions")}</DataListHead>
            </DataListHeader>
            {scrollParent && (
              <Virtuoso
                customScrollParent={scrollParent}
                data={visibleKeys}
                context={{ label: t("apiKeys.title") }}
                computeItemKey={(_index, key) => key.id}
                components={virtualDataListComponents}
                itemContent={(_index, key) => {
                  const expired =
                    key.expires_at != null && new Date(key.expires_at).getTime() < Date.now();
                  return (
                    <>
                      <DataListCell primary>
                        <div className="flex min-w-0 items-start gap-3">
                          <Checkbox
                            className="mt-0.5"
                            checked={selectedKeys.includes(key.id)}
                            aria-label={key.name}
                            onCheckedChange={() => toggleSelectKey(key.id)}
                          />
                          <div className="min-w-0 flex-1">
                            <div className="flex min-w-0 items-center gap-2 whitespace-nowrap">
                              <span
                                className={cn(
                                  "truncate font-medium",
                                  (!key.enabled || expired) && "text-muted-foreground"
                                )}
                                title={key.name}
                              >
                                {key.name}
                              </span>
                              {!key.use_user_group && key.group_ids.length > 0 && (
                                <GroupsBadge groupIds={key.group_ids} variant="secondary" />
                              )}
                            </div>
                            <div className="flex min-w-0 items-center gap-1">
                              <code className="truncate text-muted-foreground">
                                {displayedKeyPrefix(key)}…
                              </code>
                              {key.key && (
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  className="size-11 shrink-0 touch-manipulation sm:size-7"
                                  aria-label={t("common.copy")}
                                  onClick={() => handleCopy(key.key)}
                                >
                                  {copiedKey === key.key ? <Check /> : <Copy />}
                                </Button>
                              )}
                            </div>
                          </div>
                        </div>
                      </DataListCell>
                      <DataListCell label={t("apiKeys.balance")} align="end">
                        {key.sub_account_enabled ? (
                          <span className="tabular-nums">${key.sub_account_balance_usd}</span>
                        ) : (
                          <span className="whitespace-nowrap text-muted-foreground">{t("apiKeys.usesAccountBalance")}</span>
                        )}
                      </DataListCell>
                      <DataListCell label={t("apiKeys.restrictions")}>
                        <ApiKeyRestrictionBadges apiKey={key} t={t} />
                      </DataListCell>
                      <DataListCell label={t("apiKeys.expires")}>
                        <span className="inline-flex items-center gap-2 whitespace-nowrap">
                          <span className={cn("tabular-nums", expired && "text-muted-foreground")}>
                            {key.expires_at ? formatDate(key.expires_at) : t("common.never")}
                          </span>
                          {expired && <StatusBadge variant="warning">{t("apiKeys.expired")}</StatusBadge>}
                        </span>
                      </DataListCell>
                      <DataListCell label={t("common.status")}>
                        <Switch
                          className="align-middle"
                          checked={key.enabled}
                          aria-label={t("common.enableItem", { name: key.name })}
                          onCheckedChange={() => handleToggleEnabled(key)}
                        />
                      </DataListCell>
                      <DataListActions>
                        {key.sub_account_enabled && (
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-11 touch-manipulation sm:size-9"
                            aria-label={t("apiKeys.transferBalance", { defaultValue: "Transfer balance" })}
                            onClick={() => {
                              setTransferDialogKey(key);
                              setTransferAmount("");
                            }}
                          >
                            <ArrowRightLeft />
                          </Button>
                        )}
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-11 touch-manipulation sm:size-9"
                          aria-label={t("common.editItem", { name: key.name })}
                          onClick={() => openEditDialog(key)}
                        >
                          <Edit />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-11 touch-manipulation sm:size-9"
                          aria-label={t("common.deleteItem", { name: key.name })}
                          onClick={() => handleDelete(key.id)}
                        >
                          <Trash2 className="text-error-foreground" />
                        </Button>
                      </DataListActions>
                    </>
                  );
                }}
              />
            )}
          </DataList>
        )}
      </DataTableShell>

      {/* Edit Dialog */}
      <Dialog open={!!editKey} onOpenChange={(open) => { if (!open) { setEditKey(null); resetCreateForm(); } }}>
        <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-4xl">
          <div className="flex min-h-0 flex-col p-6">
          <DialogHeader className="shrink-0">
            <DialogTitle>{t("apiKeys.editApiKey")}</DialogTitle>
            <DialogDescription>
              {t("apiKeys.editDescription")}
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 space-y-4 overflow-y-auto py-4 pr-1">
            <div className="space-y-2">
              <Label htmlFor="editName">{t("common.name")}</Label>
              <Input
                id="editName"
                value={newKeyName}
                onChange={(e) => setNewKeyName(e.target.value)}
              />
            </div>
            <KeyGroupsSection
              idPrefix="edit"
              useUserGroup={newKeyUseUserGroup}
              groupIds={newKeyGroupIds}
              groups={groups}
              groupsLoading={groupsLoading}
              ownerGroupId={currentUser?.group_id ?? null}
              isAdmin={canManageSystem}
              onUseUserGroupChange={setNewKeyUseUserGroup}
              onGroupIdsChange={setNewKeyGroupIds}
            />
            <div className="flex items-center space-x-2">
              <Switch
                id="editSubAccountEnabled"
                checked={newKeySubAccountEnabled}
                onCheckedChange={setNewKeySubAccountEnabled}
              />
              <Label htmlFor="editSubAccountEnabled">{t("apiKeys.subAccountEnabled")}</Label>
            </div>
            {canManageSystem && (
              <div className="space-y-2">
                <Label htmlFor="editSubAccountBalanceNanoUsd">
                  {t("apiKeys.balance")} (nano-USD)
                </Label>
                <Input
                  id="editSubAccountBalanceNanoUsd"
                  type="text"
                  inputMode="numeric"
                  value={newKeySubAccountBalanceNanoUsd}
                  onChange={(event) => setNewKeySubAccountBalanceNanoUsd(event.target.value)}
                />
              </div>
            )}
            <div className="space-y-1">
              <div className="flex items-center space-x-2">
                <Switch
                  id="editReasoningEnvelopeEnabled"
                  checked={newKeyReasoningEnvelopeEnabled}
                  onCheckedChange={setNewKeyReasoningEnvelopeEnabled}
                />
                <Label htmlFor="editReasoningEnvelopeEnabled">{t("apiKeys.reasoningEnvelopeEnabled")}</Label>
              </div>
              <p className="text-sm text-muted-foreground">{t("apiKeys.reasoningEnvelopeHelp")}</p>
            </div>
                <div className="space-y-1">
                  <div className="flex items-center space-x-2">
                <Label htmlFor="editRequestCaptureMode">{t("apiKeys.requestCaptureMode")}</Label>
              </div>
              <Select value={newKeyRequestCaptureMode} onValueChange={(value) => setNewKeyRequestCaptureMode(value as RequestCaptureMode)}>
                <SelectTrigger id="editRequestCaptureMode">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="off">{t("apiKeys.requestCaptureModeOff")}</SelectItem>
                  <SelectItem value="capture-all">{t("apiKeys.requestCaptureModeAll")}</SelectItem>
                  <SelectItem value="capture-only-abnormal">{t("apiKeys.requestCaptureModeAbnormal")}</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-sm text-muted-foreground">{t("apiKeys.requestCaptureHelp")}</p>
            </div>
            {newKeyRequestCaptureMode !== "off" && (
              <div className="space-y-1">
                <Label htmlFor="editRequestCaptureRetention">{t("apiKeys.requestCaptureRetention")}</Label>
                <Select value={newKeyRequestCaptureRetention} onValueChange={(value) => setNewKeyRequestCaptureRetention(value as RequestCaptureRetention)}>
                  <SelectTrigger id="editRequestCaptureRetention">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="5m">{t("apiKeys.requestCaptureRetention5m")}</SelectItem>
                    <SelectItem value="1h">{t("apiKeys.requestCaptureRetention1h")}</SelectItem>
                    <SelectItem value="24h">{t("apiKeys.requestCaptureRetention24h")}</SelectItem>
                    <SelectItem value="7d">{t("apiKeys.requestCaptureRetention7d")}</SelectItem>
                  </SelectContent>
                </Select>
                <p className="text-sm text-muted-foreground">{t("apiKeys.requestCaptureRetentionHelp")}</p>
              </div>
            )}
            <div className="flex items-center space-x-2">
              <Switch
                id="editModelLimitsEnabled"
                checked={newKeyModelLimitsEnabled}
                onCheckedChange={setNewKeyModelLimitsEnabled}
              />
              <Label htmlFor="editModelLimitsEnabled">{t("apiKeys.enableModelLimits")}</Label>
            </div>
            {newKeyModelLimitsEnabled && (
              <div className="space-y-2">
                <Label htmlFor="editModelLimits">{t("apiKeys.allowedModels")}</Label>
                <Input
                  id="editModelLimits"
                  value={newKeyModelLimits}
                  onChange={(e) => setNewKeyModelLimits(e.target.value)}
                />
              </div>
            )}
            <div className="space-y-2">
              <Label htmlFor="editIpWhitelist">{t("apiKeys.ipWhitelist")}</Label>
              <Input
                id="editIpWhitelist"
                value={newKeyIpWhitelist}
                onChange={(e) => setNewKeyIpWhitelist(e.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="editMaxMultiplier">{t("apiKeys.maxMultiplier")}</Label>
              <Input
                id="editMaxMultiplier"
                type="text"
                inputMode="decimal"
                value={newKeyMaxMultiplier}
                onChange={(e) => setNewKeyMaxMultiplier(e.target.value)}
                placeholder="e.g. 1.5"
              />
              <p className="text-sm text-muted-foreground">{t("apiKeys.maxMultiplierHelp")}</p>
            </div>
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Settings2 className="h-4 w-4 text-muted-foreground" />
                <h3 className="text-sm font-medium">{t("transforms.titleApiKey")}</h3>
              </div>
              <TransformChainEditor
                value={newKeyTransforms}
                registry={apiKeyTransformRegistry}
                loading={transformRegistryLoading}
                onChange={setNewKeyTransforms}
              />
            </div>
            <ModelRedirectsEditor
              value={newKeyModelRedirects}
              onChange={setNewKeyModelRedirects}
            />
          </div>
          <DialogFooter className="shrink-0 pt-4">
            <Button variant="outline" onClick={() => { setEditKey(null); resetCreateForm(); }}>
              {t("common.cancel")}
            </Button>
            <Button onClick={handleUpdate} disabled={updating}>
              {updating ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
          </div>
        </DialogContent>
      </Dialog>

      <AlertDialog open={!!deleteTargetId} onOpenChange={(open) => { if (!open) setDeleteTargetId(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("apiKeys.confirmDeleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("apiKeys.confirmDelete")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={confirmDelete}
            >
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog open={batchDeleteOpen} onOpenChange={(open) => { if (!open) setBatchDeleteOpen(false); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("apiKeys.confirmBatchDelete", { count: selectedKeys.length })}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("apiKeys.confirmBatchDeleteDesc", { count: selectedKeys.length })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={confirmBatchDelete}
            >
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog open={!!transferDialogKey} onOpenChange={(open) => { if (!open) setTransferDialogKey(null); }}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>{t("apiKeys.transferTitle")}</DialogTitle>
            <DialogDescription>
              {t("apiKeys.transferDescription", { name: transferDialogKey?.name })}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <div className="space-y-2">
              <Label>{t("apiKeys.currentBalance")}</Label>
              <p className="text-sm font-mono">${transferDialogKey?.sub_account_balance_usd}</p>
            </div>
            <div className="space-y-2">
              <Label htmlFor="transferAmount">{t("apiKeys.transferAmount")}</Label>
              <Input
                id="transferAmount"
                type="text"
                value={transferAmount}
                onChange={(e) => setTransferAmount(e.target.value)}
                placeholder="1.00"
              />
              <p className="text-sm text-muted-foreground">{t("apiKeys.transferAmountHelp")}</p>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setTransferDialogKey(null)}>
              {t("common.cancel")}
            </Button>
            <Button
              disabled={!transferAmount || transferring}
              onClick={async () => {
                if (!transferDialogKey || !transferAmount) return;
                setTransferring(true);
                try {
                  await apiClient.transferToSubAccount(transferDialogKey.id, { amount_usd: transferAmount });
                  toast.success(t("apiKeys.transferSuccess"));
                  setTransferDialogKey(null);
                } catch (error) {
                  toast.error(error instanceof Error ? error.message : t("apiKeys.transferFailed"));
                } finally {
                  setTransferring(false);
                }
              }}
            >
              {t("apiKeys.transfer")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </PageWrapper>
  );
}
