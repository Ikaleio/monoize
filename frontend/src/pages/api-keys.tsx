import { useContext, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowRightLeft, Globe, Key, Layers, Pencil, Plus, SearchX, Settings2, Trash2 } from "lucide-react";
import { Virtuoso } from "react-virtuoso";
import { toast } from "sonner";
import { BadgeOverflowList } from "@/components/BadgeOverflowList";
import { GroupsBadge } from "@/components/GroupsBadge";
import { GroupMultiSelect } from "@/components/groups/GroupPicker";
import { ModelMultiSelect } from "@/components/models/model-multi-select";
import { ModelRedirectsEditor } from "@/components/settings/model-redirects-editor";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { findFirstInvalidTransformRule } from "@/components/transforms/transform-schema";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { CopyButton } from "@/components/ui/copy-button";
import {
  DataList,
  DataListActions,
  DataListCell,
  DataListHead,
  DataListHeader,
} from "@/components/ui/data-list";
import { virtualDataListComponents } from "@/components/ui/data-list-virtual";
import { DataTableShell, TableToolbarSearch } from "@/components/ui/data-table-shell";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { AnimatedButton, PageWrapper, motion } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { QueryError } from "@/components/ui/query-error";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { StatusBadge } from "@/components/ui/status";
import { Switch } from "@/components/ui/switch";
import { useAuth } from "@/hooks/use-auth";
import { api as apiClient } from "@/lib/api";
import type {
  ApiKey,
  ApiKeyCreated,
  CreateApiKeyInput,
  Group,
  ModelRedirectRule,
  RequestCaptureMode,
  RequestCaptureRetention,
  TransformRegistryItem,
  TransformRuleConfig,
  UpdateApiKeyInput,
} from "@/lib/api";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import { normalizeMultiplier } from "@/lib/exact-decimal";
import { formatDate } from "@/lib/format-time";
import {
  batchDeleteApiKeysOptimistic,
  createApiKeyOptimistic,
  deleteApiKeyOptimistic,
  updateApiKeyOptimistic,
  useApiKeys,
  useDashboardGroups,
  useMarketplaceModels,
  useTransformRegistry,
} from "@/lib/swr";
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

interface KeyForm {
  name: string;
  expiresInDays: string;
  subAccountEnabled: boolean;
  subAccountBalanceNanoUsd: string;
  modelLimitsEnabled: boolean;
  modelLimits: string[];
  ipWhitelist: string;
  useUserGroup: boolean;
  groupIds: string[];
  maxMultiplier: string;
  transforms: TransformRuleConfig[];
  modelRedirects: ModelRedirectRule[];
  reasoningEnvelopeEnabled: boolean;
  requestCaptureMode: RequestCaptureMode;
  requestCaptureRetention: RequestCaptureRetention;
}

const EMPTY_KEY_FORM: KeyForm = {
  name: "",
  expiresInDays: "",
  subAccountEnabled: false,
  subAccountBalanceNanoUsd: "0",
  modelLimitsEnabled: false,
  modelLimits: [],
  ipWhitelist: "",
  useUserGroup: true,
  groupIds: [],
  maxMultiplier: "",
  transforms: [],
  modelRedirects: [],
  reasoningEnvelopeEnabled: true,
  requestCaptureMode: "off",
  requestCaptureRetention: "24h",
};

function formFromKey(key: ApiKey): KeyForm {
  return {
    name: key.name,
    expiresInDays: "",
    subAccountEnabled: key.sub_account_enabled,
    subAccountBalanceNanoUsd: key.sub_account_balance_nano_usd,
    modelLimitsEnabled: key.model_limits_enabled,
    modelLimits: key.model_limits,
    ipWhitelist: key.ip_whitelist.join(", "),
    useUserGroup: key.use_user_group,
    groupIds: key.group_ids ?? [],
    maxMultiplier: key.max_multiplier != null ? String(key.max_multiplier) : "",
    transforms: key.transforms ?? [],
    modelRedirects: key.model_redirects ?? [],
    reasoningEnvelopeEnabled: key.reasoning_envelope_enabled ?? true,
    requestCaptureMode: key.request_capture_mode ?? "off",
    requestCaptureRetention: key.request_capture_retention ?? "24h",
  };
}

type KeyDialogTarget = { mode: "create" } | { mode: "edit"; key: ApiKey };

interface KeyFormDialogProps {
  target: KeyDialogTarget | null;
  form: KeyForm;
  onFormChange: (patch: Partial<KeyForm>) => void;
  onOpenChange: (open: boolean) => void;
  onSubmit: () => void;
  submitting: boolean;
  canManageSystem: boolean;
  ownerGroupId: string | null;
  groups: Group[];
  groupsLoading: boolean;
  transformRegistry: TransformRegistryItem[];
  transformRegistryLoading: boolean;
}

/** TM-UI4: one form for both create and edit; only the expiry field is create-only. */
function KeyFormDialog({
  target,
  form,
  onFormChange,
  onOpenChange,
  onSubmit,
  submitting,
  canManageSystem,
  ownerGroupId,
  groups,
  groupsLoading,
  transformRegistry,
  transformRegistryLoading,
}: KeyFormDialogProps) {
  const { t } = useTranslation();
  const isCreate = target?.mode === "create";
  const {
    data: marketplaceModels,
    error: marketplaceError,
    isLoading: marketplaceLoading,
    mutate: reloadMarketplace,
  } = useMarketplaceModels({ isPaused: () => !target || !form.modelLimitsEnabled });
  const modelOptions = useMemo(
    () => (marketplaceModels ?? []).map((record) => record.model_id),
    [marketplaceModels]
  );

  return (
    <Dialog open={target !== null} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-4xl">
        <div className="flex min-h-0 flex-col p-6">
          <DialogHeader className="shrink-0">
            <DialogTitle>{t(isCreate ? "apiKeys.createApiKey" : "apiKeys.editApiKey")}</DialogTitle>
            <DialogDescription>
              {t(isCreate ? "apiKeys.createDescription" : "apiKeys.editDescription")}
            </DialogDescription>
          </DialogHeader>
          <div className="-mx-1 min-h-0 flex-1 space-y-4 overflow-y-auto px-1 py-4">
            <div className="space-y-2">
              <Label htmlFor="key-name">{t("common.name")}</Label>
              <Input
                id="key-name"
                value={form.name}
                onChange={(event) => onFormChange({ name: event.target.value })}
                placeholder="My API Key"
              />
            </div>
            {isCreate && (
              <div className="space-y-2">
                <Label htmlFor="key-expires">{t("apiKeys.expiresInDays")}</Label>
                <Input
                  id="key-expires"
                  type="number"
                  min="1"
                  value={form.expiresInDays}
                  onChange={(event) => onFormChange({ expiresInDays: event.target.value })}
                  placeholder="30"
                />
              </div>
            )}
            <KeyGroupsSection
              idPrefix="key"
              useUserGroup={form.useUserGroup}
              groupIds={form.groupIds}
              groups={groups}
              groupsLoading={groupsLoading}
              ownerGroupId={ownerGroupId}
              isAdmin={canManageSystem}
              onUseUserGroupChange={(useUserGroup) => onFormChange({ useUserGroup })}
              onGroupIdsChange={(groupIds) => onFormChange({ groupIds })}
            />
            <div className="flex items-center gap-2">
              <Switch
                id="key-sub-account"
                checked={form.subAccountEnabled}
                onCheckedChange={(subAccountEnabled) => onFormChange({ subAccountEnabled })}
              />
              <Label htmlFor="key-sub-account">{t("apiKeys.subAccountEnabled")}</Label>
            </div>
            {canManageSystem && (
              <div className="space-y-2">
                <Label htmlFor="key-sub-account-balance">{t("apiKeys.balance")} (nano-USD)</Label>
                <Input
                  id="key-sub-account-balance"
                  type="text"
                  inputMode="numeric"
                  value={form.subAccountBalanceNanoUsd}
                  onChange={(event) => onFormChange({ subAccountBalanceNanoUsd: event.target.value })}
                />
              </div>
            )}
            <div className="space-y-1">
              <div className="flex items-center gap-2">
                <Switch
                  id="key-reasoning-envelope"
                  checked={form.reasoningEnvelopeEnabled}
                  onCheckedChange={(reasoningEnvelopeEnabled) => onFormChange({ reasoningEnvelopeEnabled })}
                />
                <Label htmlFor="key-reasoning-envelope">{t("apiKeys.reasoningEnvelopeEnabled")}</Label>
              </div>
              <p className="text-sm text-muted-foreground">{t("apiKeys.reasoningEnvelopeHelp")}</p>
            </div>
            <div className="space-y-1">
              <Label htmlFor="key-capture-mode">{t("apiKeys.requestCaptureMode")}</Label>
              <Select
                value={form.requestCaptureMode}
                onValueChange={(value) => onFormChange({ requestCaptureMode: value as RequestCaptureMode })}
              >
                <SelectTrigger id="key-capture-mode">
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
            {form.requestCaptureMode !== "off" && (
              <div className="space-y-1">
                <Label htmlFor="key-capture-retention">{t("apiKeys.requestCaptureRetention")}</Label>
                <Select
                  value={form.requestCaptureRetention}
                  onValueChange={(value) =>
                    onFormChange({ requestCaptureRetention: value as RequestCaptureRetention })
                  }
                >
                  <SelectTrigger id="key-capture-retention">
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
            <div className="flex items-center gap-2">
              <Switch
                id="key-model-limits"
                checked={form.modelLimitsEnabled}
                onCheckedChange={(modelLimitsEnabled) => onFormChange({ modelLimitsEnabled })}
              />
              <Label htmlFor="key-model-limits">{t("apiKeys.enableModelLimits")}</Label>
            </div>
            {form.modelLimitsEnabled && (
              <div className="space-y-2">
                <Label>{t("apiKeys.allowedModels")}</Label>
                <ModelMultiSelect
                  value={form.modelLimits}
                  options={modelOptions}
                  loading={marketplaceLoading}
                  error={marketplaceError}
                  onRetry={() => reloadMarketplace()}
                  onChange={(modelLimits) => onFormChange({ modelLimits })}
                  allowCustom
                  label={t("apiKeys.allowedModels")}
                />
                <p className="text-sm text-muted-foreground">{t("apiKeys.modelsHelp")}</p>
              </div>
            )}
            <div className="space-y-2">
              <Label htmlFor="key-ip-whitelist">{t("apiKeys.ipWhitelist")}</Label>
              <Input
                id="key-ip-whitelist"
                value={form.ipWhitelist}
                onChange={(event) => onFormChange({ ipWhitelist: event.target.value })}
                placeholder="192.168.1.1, 10.0.0.0/8"
              />
              <p className="text-sm text-muted-foreground">{t("apiKeys.ipHelp")}</p>
            </div>
            <div className="space-y-2">
              <Label htmlFor="key-max-multiplier">{t("apiKeys.maxMultiplier")}</Label>
              <Input
                id="key-max-multiplier"
                type="text"
                inputMode="decimal"
                value={form.maxMultiplier}
                onChange={(event) => onFormChange({ maxMultiplier: event.target.value })}
                placeholder="e.g. 1.5"
              />
              <p className="text-sm text-muted-foreground">{t("apiKeys.maxMultiplierHelp")}</p>
            </div>
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Settings2 className="size-4 text-muted-foreground" aria-hidden="true" />
                <h3 className="text-sm font-medium">{t("transforms.titleApiKey")}</h3>
              </div>
              <TransformChainEditor
                value={form.transforms}
                registry={transformRegistry}
                loading={transformRegistryLoading}
                onChange={(transforms) => onFormChange({ transforms })}
              />
            </div>
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <ArrowRightLeft className="size-4 text-muted-foreground" aria-hidden="true" />
                <h3 className="text-sm font-medium">{t("apiKeys.modelRedirects")}</h3>
              </div>
              <ModelRedirectsEditor
                value={form.modelRedirects}
                emptyText={t("apiKeys.modelRedirectsEmpty")}
                onChange={(modelRedirects) => onFormChange({ modelRedirects })}
              />
            </div>
          </div>
          <DialogFooter className="shrink-0 pt-4">
            <Button variant="outline" onClick={() => onOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button onClick={onSubmit} disabled={submitting || !form.name.trim()}>
              {isCreate
                ? t(submitting ? "common.creating" : "common.create")
                : t(submitting ? "common.saving" : "common.save")}
            </Button>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );
}

export function ApiKeysPage() {
  const { t } = useTranslation();
  const { user: currentUser } = useAuth();
  const { data, error, isLoading, isValidating, mutate } = useApiKeys();
  const keys = useMemo(() => data ?? [], [data]);
  const { data: groups = [], isLoading: groupsLoading } = useDashboardGroups();
  const canManageSystem = currentUser?.role === "admin" || currentUser?.role === "super_admin";
  // /transforms/registry is admin-only; skip it for non-admins to avoid a 403 loop.
  const { data: transformRegistry = [], isLoading: transformRegistryLoading } =
    useTransformRegistry({ isPaused: () => !canManageSystem });
  const apiKeyTransformRegistry = useMemo(
    () => transformRegistry.filter((item) => item.supported_scopes.includes("api_key")),
    [transformRegistry]
  );
  const [dialogTarget, setDialogTarget] = useState<KeyDialogTarget | null>(null);
  const [form, setForm] = useState<KeyForm>(EMPTY_KEY_FORM);
  const [submitting, setSubmitting] = useState(false);
  const [createdKey, setCreatedKey] = useState<ApiKeyCreated | null>(null);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [deleteTarget, setDeleteTarget] = useState<ApiKey | null>(null);
  const [batchDeleteOpen, setBatchDeleteOpen] = useState(false);
  const [transferDialogKey, setTransferDialogKey] = useState<ApiKey | null>(null);
  const [transferAmount, setTransferAmount] = useState("");
  const [transferring, setTransferring] = useState(false);
  const [query, setQuery] = useState("");
  const scrollParent = useContext(DashboardScrollParentContext);
  const needle = query.trim().toLowerCase();
  const visibleKeys = useMemo(
    () => keys.filter((key) => keyMatchesQuery(key, needle)),
    [keys, needle]
  );

  const openCreate = () => {
    setForm(EMPTY_KEY_FORM);
    setDialogTarget({ mode: "create" });
  };

  const openEdit = (key: ApiKey) => {
    setForm(formFromKey(key));
    setDialogTarget({ mode: "edit", key });
  };

  const handleSubmit = async () => {
    if (!dialogTarget || !form.name.trim()) return;
    const invalidRule = findFirstInvalidTransformRule(form.transforms, apiKeyTransformRegistry);
    if (invalidRule) {
      const firstError = invalidRule.errors[0];
      toast.error(t("transforms.validationRuleInvalid", {
        index: invalidRule.index + 1,
        reason: `${firstError.field} ${firstError.message}`,
      }));
      return;
    }
    if (!form.useUserGroup && form.groupIds.length === 0) {
      toast.error(t("apiKeys.groupsRequired"));
      return;
    }
    setSubmitting(true);
    try {
      const shared = {
        sub_account_enabled: form.subAccountEnabled,
        model_limits_enabled: form.modelLimitsEnabled,
        model_limits: form.modelLimits,
        ip_whitelist: form.ipWhitelist.split(",").map((item) => item.trim()).filter(Boolean),
        use_user_group: form.useUserGroup,
        group_ids: form.useUserGroup ? [] : form.groupIds,
        max_multiplier: parseOptionalMultiplier(form.maxMultiplier),
        transforms: form.transforms,
        model_redirects: form.modelRedirects.filter((rule) => rule.pattern.trim() && rule.replace.trim()),
        reasoning_envelope_enabled: form.reasoningEnvelopeEnabled,
        request_capture_mode: form.requestCaptureMode,
        request_capture_retention: form.requestCaptureRetention,
      };
      if (dialogTarget.mode === "create") {
        const initialSubAccountBalance = canManageSystem
          ? parseOptionalNanoBalance(form.subAccountBalanceNanoUsd, false)
          : undefined;
        if (
          !form.subAccountEnabled
          && initialSubAccountBalance != null
          && BigInt(initialSubAccountBalance) !== 0n
        ) {
          throw new Error("A non-zero initial balance requires sub-account billing to be enabled");
        }
        const input: CreateApiKeyInput = {
          ...shared,
          name: form.name.trim(),
          expires_in_days: form.expiresInDays ? parseInt(form.expiresInDays) : undefined,
          ...(canManageSystem ? { sub_account_balance_nano_usd: initialSubAccountBalance } : {}),
        };
        setCreatedKey(await createApiKeyOptimistic(input, keys));
      } else {
        const input: UpdateApiKeyInput = {
          ...shared,
          name: form.name.trim(),
          ...(canManageSystem && form.subAccountEnabled
            ? { sub_account_balance_nano_usd: parseOptionalNanoBalance(form.subAccountBalanceNanoUsd) }
            : {}),
        };
        await updateApiKeyOptimistic(dialogTarget.key.id, input, keys);
        toast.success(t("apiKeys.updateSuccess"));
      }
      setDialogTarget(null);
    } catch (error) {
      const fallback = dialogTarget.mode === "create" ? "apiKeys.failedCreate" : "apiKeys.failedUpdate";
      toast.error(error instanceof Error ? error.message : t(fallback));
    } finally {
      setSubmitting(false);
    }
  };

  const confirmDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deleteApiKeyOptimistic(deleteTarget.id, keys);
      setSelectedKeys((prev) => prev.filter((id) => id !== deleteTarget.id));
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedDelete"));
    }
  };

  const confirmBatchDelete = async () => {
    try {
      await batchDeleteApiKeysOptimistic(selectedKeys, keys);
      setSelectedKeys([]);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedBatchDelete"));
    }
  };

  const handleToggleEnabled = async (key: ApiKey) => {
    try {
      await updateApiKeyOptimistic(key.id, { enabled: !key.enabled }, keys);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("apiKeys.failedUpdate"));
    }
  };

  const handleTransfer = async () => {
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
  };

  const toggleSelectKey = (id: string) => {
    setSelectedKeys((prev) =>
      prev.includes(id) ? prev.filter((selectedId) => selectedId !== id) : [...prev, id]
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

  if (isLoading) {
    return (
      <PageWrapper className="space-y-6">
        <TablePageSkeleton />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <PageHeader
        title={t("apiKeys.title")}
        description={t("apiKeys.description")}
        actions={(
          <>
            {selectedKeys.length > 0 && (
              <Button variant="destructive" onClick={() => setBatchDeleteOpen(true)}>
                <Trash2 data-icon="inline-start" aria-hidden="true" />
                {t("apiKeys.batchDelete")}
              </Button>
            )}
            <AnimatedButton>
              <Button onClick={openCreate}>
                <Plus data-icon="inline-start" aria-hidden="true" />
                {t("apiKeys.createKey")}
              </Button>
            </AnimatedButton>
          </>
        )}
      />

      {error ? <QueryError onRetry={mutate} retrying={isValidating} stale={data !== undefined} /> : null}

      {createdKey && (
        <motion.div
          initial={{ opacity: 0, y: 20, scale: 0.95 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ type: "spring", stiffness: 300, damping: 25 }}
        >
          <Card className="border-success-border bg-success-soft">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-success-foreground">
                <Key className="size-5" aria-hidden="true" />
                {t("apiKeys.apiKeyCreated")}
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="flex items-center gap-2">
                <code className="min-w-0 flex-1 break-all rounded-lg border bg-muted px-3 py-2 text-sm">
                  {createdKey.key}
                </code>
                <CopyButton value={createdKey.key} />
              </div>
              <Button variant="ghost" size="sm" className="mt-2" onClick={() => setCreatedKey(null)}>
                {t("common.dismiss")}
              </Button>
            </CardContent>
          </Card>
        </motion.div>
      )}

      {data !== undefined ? (
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
                <Button onClick={openCreate}>
                  <Plus data-icon="inline-start" aria-hidden="true" />
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
                                {key.key && <CopyButton value={key.key} />}
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
                            onClick={() => openEdit(key)}
                          >
                            <Pencil />
                          </Button>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-11 touch-manipulation sm:size-9"
                            aria-label={t("common.deleteItem", { name: key.name })}
                            onClick={() => setDeleteTarget(key)}
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
      ) : null}

      <KeyFormDialog
        target={dialogTarget}
        form={form}
        onFormChange={(patch) => setForm((prev) => ({ ...prev, ...patch }))}
        onOpenChange={(open) => {
          if (!open) setDialogTarget(null);
        }}
        onSubmit={() => void handleSubmit()}
        submitting={submitting}
        canManageSystem={canManageSystem}
        ownerGroupId={currentUser?.group_id ?? null}
        groups={groups}
        groupsLoading={groupsLoading}
        transformRegistry={apiKeyTransformRegistry}
        transformRegistryLoading={transformRegistryLoading}
      />

      <ConfirmDeleteDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => {
          if (!open) setDeleteTarget(null);
        }}
        title={t("apiKeys.confirmDeleteTitle")}
        description={t("apiKeys.confirmDelete", { name: deleteTarget?.name })}
        onConfirm={confirmDelete}
      />

      <ConfirmDeleteDialog
        open={batchDeleteOpen}
        onOpenChange={setBatchDeleteOpen}
        title={t("apiKeys.confirmBatchDelete", { count: selectedKeys.length })}
        description={t("apiKeys.confirmBatchDeleteDesc", { count: selectedKeys.length })}
        onConfirm={confirmBatchDelete}
      />

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
              <p className="text-sm tabular-nums">${transferDialogKey?.sub_account_balance_usd}</p>
            </div>
            <div className="space-y-2">
              <Label htmlFor="transferAmount">{t("apiKeys.transferAmount")}</Label>
              <Input
                id="transferAmount"
                type="text"
                value={transferAmount}
                onChange={(event) => setTransferAmount(event.target.value)}
                placeholder="1.00"
              />
              <p className="text-sm text-muted-foreground">{t("apiKeys.transferAmountHelp")}</p>
            </div>
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => setTransferDialogKey(null)}>
              {t("common.cancel")}
            </Button>
            <Button disabled={!transferAmount || transferring} onClick={() => void handleTransfer()}>
              {t("apiKeys.transfer")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </PageWrapper>
  );
}
