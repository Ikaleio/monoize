import { useContext, useId, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowRightLeft,
  ChevronDown,
  Coins,
  Globe,
  Key,
  Layers,
  Pencil,
  Plus,
  SearchX,
  Settings2,
  Trash2,
} from "lucide-react";
import { Virtuoso } from "react-virtuoso";
import { toast } from "sonner";
import { BadgeOverflowList } from "@/components/BadgeOverflowList";
import { GroupsBadge } from "@/components/GroupsBadge";
import { GroupMultiSelect } from "@/components/groups/GroupPicker";
import { ModelMultiSelect } from "@/components/models/model-multi-select";
import { ModelRedirectsEditor } from "@/components/settings/model-redirects-editor";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { revealTransformRule } from "@/components/transforms/transform-layer";
import {
  findFirstInvalidTransformRule,
  formatTransformRuleError,
} from "@/components/transforms/transform-schema";
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
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ListField } from "@/components/ui/list-field";
import { AnimatedButton, PageWrapper, motion } from "@/components/ui/motion";
import { NumberField } from "@/components/ui/number-field";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { QueryError } from "@/components/ui/query-error";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { StatusBadge } from "@/components/ui/status";
import { Switch } from "@/components/ui/switch";
import { useAuth } from "@/hooks/use-auth";
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
import { DashboardApiError } from "@/lib/api";
import { apiErrorMessage, apiErrorParam, toastApiError } from "@/lib/api-error";
import { DashboardScrollParentContext } from "@/lib/dashboard-scroll";
import {
  formatNanoUsd,
  formatUsdDecimal,
  normalizeMultiplier,
  usdDecimalToNano,
} from "@/lib/exact-decimal";
import { formatDate } from "@/lib/format-time";
import {
  batchDeleteApiKeysOptimistic,
  createApiKeyOptimistic,
  deleteApiKeyOptimistic,
  transferToSubAccountAndRefresh,
  updateApiKeyOptimistic,
  useApiKeys,
  useDashboardGroups,
  useMarketplaceModels,
  usePublicSettings,
  useTransformRegistry,
} from "@/lib/swr";
import { cn } from "@/lib/utils";
import { resolveApiBase } from "@/pages/dashboard/utils";

function requestCaptureBadgeVariant(mode: RequestCaptureMode): "secondary" | "outline" {
  return mode === "capture-only-abnormal" ? "secondary" : "outline";
}

function ApiKeyRestrictionBadges({
  apiKey,
  t,
  empty = <span className="text-muted-foreground">—</span>,
}: {
  apiKey: ApiKey;
  t: (key: string) => string;
  /** Rendered when the key has no restriction. */
  empty?: ReactNode;
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
    return empty;
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

// Container-query visibility for the key list (AK-UI-LIST-2); 56rem is the DataList wide threshold.
const WIDE_ONLY = "hidden [@container(min-width:56rem)]:block";
const NARROW_ONLY = "[@container(min-width:56rem)]:hidden";

/**
 * Wide-mode tracks for the key list (AK-UI-LIST-1): name, key, restrictions, expiry,
 * optional sub-account balance, status, actions. The actions track fits the most
 * row buttons that can occur (design.md §4.3). The key track fits the 12-character
 * prefix and its copy button, and restrictions keep room for two collapsed badges, so
 * near the 56rem wide threshold only the name column truncates.
 */
function tokenListColumns(showSubAccount: boolean, isAdmin: boolean): string {
  const actionCount = showSubAccount ? (isAdmin ? 4 : 3) : 2;
  const actionsRem = Math.ceil((actionCount * 2.25 + (actionCount - 1) * 0.25) * 2) / 2;
  return [
    "minmax(0,3fr)",
    "8.5rem",
    "minmax(5.5rem,1fr)",
    "9rem",
    ...(showSubAccount ? ["8rem"] : []),
    "3rem",
    `${actionsRem}rem`,
  ].join(" ");
}

/** `YYYY-MM-DD` of an instant in the browser's time zone, for `<input type="date">`. */
function localDateInputValue(iso: string): string {
  const date = new Date(iso);
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** TM-UI4: an edited expiry ends with the selected local day. */
function endOfLocalDayIso(date: string): string {
  return new Date(`${date}T23:59:59.999`).toISOString();
}

/** One read-only "API base URL" line (TM-UI8). */
function ApiBaseUrlRow() {
  const { t } = useTranslation();
  const { data: settings } = usePublicSettings();
  const { url, isFallback } = resolveApiBase(settings);
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 text-sm">
      <span className="text-muted-foreground">
        {t(isFallback ? "apiKeys.apiBaseUrlFallback" : "apiKeys.apiBaseUrl")}
      </span>
      <span className="flex min-w-0 items-center gap-1">
        <code className="truncate">{url}</code>
        <CopyButton value={url} label={t("apiKeys.copyApiBaseUrl")} />
      </span>
    </div>
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
  error?: string;
  onUseUserGroupChange: (next: boolean) => void;
  onGroupIdsChange: (next: string[]) => void;
}

/**
 * TM-GRP-1..TM-GRP-5: a key either follows the owner's single group or holds
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
  error,
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
        <Field data-invalid={error ? true : undefined}>
          <FieldLabel id={`${idPrefix}-groups-label`}>{t("apiKeys.groups")}</FieldLabel>
          <div id={`${idPrefix}-groups`} tabIndex={-1} aria-labelledby={`${idPrefix}-groups-label`}>
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
          </div>
          <FieldDescription>{t("apiKeys.groupsHelp")}</FieldDescription>
          {error && <FieldError>{error}</FieldError>}
        </Field>
      )}
    </div>
  );
}

interface KeyForm {
  name: string;
  /** Create only; null = never expires. */
  expiresInDays: number | null;
  /** Edit only (TM-UI4). */
  expiryNever: boolean;
  expiryDate: string;
  subAccountEnabled: boolean;
  /** Create only, admins only: USD decimal (TM-UI1). */
  initialBalanceUsd: string;
  modelLimitsEnabled: boolean;
  modelLimits: string[];
  ipWhitelist: string[];
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
  expiresInDays: null,
  expiryNever: true,
  expiryDate: "",
  subAccountEnabled: false,
  initialBalanceUsd: "0",
  modelLimitsEnabled: false,
  modelLimits: [],
  ipWhitelist: [],
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
    ...EMPTY_KEY_FORM,
    name: key.name,
    expiryNever: !key.expires_at,
    expiryDate: key.expires_at ? localDateInputValue(key.expires_at) : "",
    subAccountEnabled: key.sub_account_enabled,
    modelLimitsEnabled: key.model_limits_enabled,
    modelLimits: key.model_limits,
    ipWhitelist: key.ip_whitelist,
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

/** TM-UI5: advanced items that differ from their create defaults. */
function countConfiguredAdvanced(form: KeyForm): number {
  return [
    !form.reasoningEnvelopeEnabled,
    form.requestCaptureMode !== "off",
    form.modelLimitsEnabled,
    form.ipWhitelist.length > 0,
    form.maxMultiplier.trim() !== "",
    form.transforms.length > 0,
    form.modelRedirects.some((rule) => rule.pattern.trim() && rule.replace.trim()),
  ].filter(Boolean).length;
}

type KeyFormField = "name" | "expiry" | "initialBalance" | "groups" | "maxMultiplier" | "transforms";
type KeyFormErrors = Partial<Record<KeyFormField, string>>;

const FIELD_ORDER: readonly KeyFormField[] = [
  "name",
  "expiry",
  "groups",
  "initialBalance",
  "maxMultiplier",
  "transforms",
];
const ADVANCED_FIELDS: Partial<Record<KeyFormField, true>> = {
  maxMultiplier: true,
  transforms: true,
};
const FIELD_ELEMENT_IDS: Record<KeyFormField, string> = {
  name: "key-name",
  expiry: "key-expires",
  initialBalance: "key-initial-balance",
  groups: "key-groups",
  maxMultiplier: "key-max-multiplier",
  transforms: "key-transforms",
};

type KeyDialogTarget = { mode: "create" } | { mode: "edit"; key: ApiKey };

interface KeyFormDialogProps {
  target: KeyDialogTarget | null;
  form: KeyForm;
  initialForm: KeyForm;
  errors: KeyFormErrors;
  expiryDaysValid: boolean;
  advancedOpen: boolean;
  onAdvancedOpenChange: (open: boolean) => void;
  onExpiryDaysValidityChange: (valid: boolean) => void;
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

/** TM-UI4/TM-UI5: one form for create and edit; basic fields first, the rest collapsed. */
function KeyFormDialog({
  target,
  form,
  initialForm,
  errors,
  expiryDaysValid,
  advancedOpen,
  onAdvancedOpenChange,
  onExpiryDaysValidityChange,
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
  const advancedId = useId();
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
  const dirty = JSON.stringify(form) !== JSON.stringify(initialForm);
  const configuredCount = countConfiguredAdvanced(form);
  const initialBalanceNano = usdDecimalToNano(form.initialBalanceUsd);

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    onSubmit();
  };

  return (
    <Dialog open={target !== null} onOpenChange={onOpenChange}>
      <DialogContent
        dirty={dirty}
        className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-3xl"
      >
        <form noValidate onSubmit={handleSubmit} className="flex min-h-0 flex-col p-6">
          <DialogHeader className="shrink-0">
            <DialogTitle>{t(isCreate ? "apiKeys.createApiKey" : "apiKeys.editApiKey")}</DialogTitle>
            <DialogDescription>
              {t(isCreate ? "apiKeys.createDescription" : "apiKeys.editDescription")}
            </DialogDescription>
          </DialogHeader>
          <div className="-mx-1 min-h-0 flex-1 space-y-4 overflow-y-auto px-1 py-4">
            <Field data-invalid={errors.name ? true : undefined}>
              <FieldLabel htmlFor="key-name">{t("common.name")}</FieldLabel>
              <Input
                id="key-name"
                value={form.name}
                aria-invalid={errors.name ? true : undefined}
                onChange={(event) => onFormChange({ name: event.target.value })}
                placeholder={t("apiKeys.namePlaceholder")}
              />
              {errors.name && <FieldError>{errors.name}</FieldError>}
            </Field>
            {isCreate ? (
              <Field data-invalid={errors.expiry || !expiryDaysValid ? true : undefined}>
                <FieldLabel htmlFor="key-expires">{t("apiKeys.expiresInDays")}</FieldLabel>
                <NumberField
                  id="key-expires"
                  value={form.expiresInDays}
                  onValueChange={(expiresInDays) => onFormChange({ expiresInDays })}
                  onValidityChange={onExpiryDaysValidityChange}
                  min={1}
                  allowEmpty
                  unit={t("apiKeys.daysUnit")}
                  placeholder="30"
                />
                <FieldDescription>{t("apiKeys.expiresInDaysHelp")}</FieldDescription>
                {errors.expiry && <FieldError>{errors.expiry}</FieldError>}
              </Field>
            ) : (
              <Field data-invalid={errors.expiry ? true : undefined}>
                <FieldLabel htmlFor="key-expires">{t("apiKeys.expiryDate")}</FieldLabel>
                <div className="flex flex-wrap items-center gap-3">
                  <Input
                    id="key-expires"
                    type="date"
                    className="w-auto"
                    value={form.expiryDate}
                    disabled={form.expiryNever}
                    aria-invalid={errors.expiry ? true : undefined}
                    onChange={(event) => onFormChange({ expiryDate: event.target.value })}
                  />
                  <div className="flex items-center gap-2">
                    <Checkbox
                      id="key-expiry-never"
                      checked={form.expiryNever}
                      onCheckedChange={(checked) => onFormChange({ expiryNever: checked === true })}
                    />
                    <Label htmlFor="key-expiry-never" className="font-normal">
                      {t("apiKeys.expiryNever")}
                    </Label>
                  </div>
                </div>
                <FieldDescription>{t("apiKeys.expiryDateHelp")}</FieldDescription>
                {errors.expiry && <FieldError>{errors.expiry}</FieldError>}
              </Field>
            )}
            <KeyGroupsSection
              idPrefix="key"
              useUserGroup={form.useUserGroup}
              groupIds={form.groupIds}
              groups={groups}
              groupsLoading={groupsLoading}
              ownerGroupId={ownerGroupId}
              isAdmin={canManageSystem}
              error={errors.groups}
              onUseUserGroupChange={(useUserGroup) => onFormChange({ useUserGroup })}
              onGroupIdsChange={(groupIds) => onFormChange({ groupIds })}
            />
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Switch
                  id="key-sub-account"
                  checked={form.subAccountEnabled}
                  onCheckedChange={(subAccountEnabled) => onFormChange({ subAccountEnabled })}
                />
                <Label htmlFor="key-sub-account">{t("apiKeys.subAccountEnabled")}</Label>
              </div>
              <p className="text-sm text-muted-foreground">{t("apiKeys.subAccountHelp")}</p>
              {isCreate && canManageSystem && form.subAccountEnabled && (
                <Field data-invalid={errors.initialBalance ? true : undefined}>
                  <FieldLabel htmlFor="key-initial-balance">{t("apiKeys.initialBalance")}</FieldLabel>
                  <Input
                    id="key-initial-balance"
                    inputMode="decimal"
                    value={form.initialBalanceUsd}
                    aria-invalid={errors.initialBalance ? true : undefined}
                    onChange={(event) => onFormChange({ initialBalanceUsd: event.target.value })}
                    placeholder="0.00"
                  />
                  <FieldDescription className="tabular-nums">
                    {initialBalanceNano !== null
                      ? t("apiKeys.initialBalanceNano", { nano: initialBalanceNano.toString() })
                      : t("apiKeys.initialBalanceHelp")}
                  </FieldDescription>
                  {errors.initialBalance && <FieldError>{errors.initialBalance}</FieldError>}
                </Field>
              )}
            </div>

            <div className="rounded-lg border">
              <button
                type="button"
                className="flex w-full items-center justify-between gap-3 rounded-lg px-3 py-2.5 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                aria-expanded={advancedOpen}
                aria-controls={advancedId}
                onClick={() => onAdvancedOpenChange(!advancedOpen)}
              >
                <span className="text-sm font-medium">{t("apiKeys.advancedSettings")}</span>
                <span className="flex items-center gap-2 text-sm text-muted-foreground">
                  {t("apiKeys.advancedConfigured", { count: configuredCount })}
                  <ChevronDown
                    className={cn("size-4 transition-transform", advancedOpen && "rotate-180")}
                    aria-hidden="true"
                  />
                </span>
              </button>
              {advancedOpen && (
                <div id={advancedId} className="space-y-4 border-t p-3">
                  <div className="space-y-1">
                    <div className="flex items-center gap-2">
                      <Switch
                        id="key-reasoning-envelope"
                        checked={form.reasoningEnvelopeEnabled}
                        onCheckedChange={(reasoningEnvelopeEnabled) =>
                          onFormChange({ reasoningEnvelopeEnabled })
                        }
                      />
                      <Label htmlFor="key-reasoning-envelope">{t("apiKeys.reasoningEnvelopeEnabled")}</Label>
                    </div>
                    <p className="text-sm text-muted-foreground">{t("apiKeys.reasoningEnvelopeHelp")}</p>
                  </div>
                  <div className="space-y-1">
                    <Label htmlFor="key-capture-mode">{t("apiKeys.requestCaptureMode")}</Label>
                    <Select
                      value={form.requestCaptureMode}
                      onValueChange={(value) =>
                        onFormChange({ requestCaptureMode: value as RequestCaptureMode })
                      }
                    >
                      <SelectTrigger id="key-capture-mode">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="off">{t("apiKeys.requestCaptureModeOff")}</SelectItem>
                        <SelectItem value="capture-all">{t("apiKeys.requestCaptureModeAll")}</SelectItem>
                        <SelectItem value="capture-only-abnormal">
                          {t("apiKeys.requestCaptureModeAbnormal")}
                        </SelectItem>
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
                      <p className="text-sm text-muted-foreground">
                        {t("apiKeys.requestCaptureRetentionHelp")}
                      </p>
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
                  <Field>
                    <FieldLabel htmlFor="key-ip-whitelist">{t("apiKeys.ipWhitelist")}</FieldLabel>
                    <ListField
                      id="key-ip-whitelist"
                      value={form.ipWhitelist}
                      onValueChange={(ipWhitelist) => onFormChange({ ipWhitelist })}
                      placeholder="192.168.1.1, 10.0.0.0/8"
                    />
                    <FieldDescription>{t("apiKeys.ipHelp")}</FieldDescription>
                  </Field>
                  <Field data-invalid={errors.maxMultiplier ? true : undefined}>
                    <FieldLabel htmlFor="key-max-multiplier">{t("apiKeys.maxMultiplier")}</FieldLabel>
                    <Input
                      id="key-max-multiplier"
                      inputMode="decimal"
                      value={form.maxMultiplier}
                      aria-invalid={errors.maxMultiplier ? true : undefined}
                      onChange={(event) => onFormChange({ maxMultiplier: event.target.value })}
                      placeholder={t("apiKeys.maxMultiplierPlaceholder")}
                    />
                    <FieldDescription>{t("apiKeys.maxMultiplierHelp")}</FieldDescription>
                    {errors.maxMultiplier && <FieldError>{errors.maxMultiplier}</FieldError>}
                  </Field>
                  {canManageSystem && (
                    <div id="key-transforms" tabIndex={-1} className="space-y-3">
                      <div className="flex items-center gap-2">
                        <Settings2 className="size-4 text-muted-foreground" aria-hidden="true" />
                        <h3 className="text-sm font-medium">{t("transforms.titleApiKey")}</h3>
                      </div>
                      <TransformChainEditor
                        layer="apiKey"
                        value={form.transforms}
                        registry={transformRegistry}
                        loading={transformRegistryLoading}
                        onChange={(transforms) => onFormChange({ transforms })}
                      />
                      {errors.transforms && <FieldError>{errors.transforms}</FieldError>}
                    </div>
                  )}
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
              )}
            </div>
          </div>
          <DialogFooter className="shrink-0 pt-4">
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={submitting}>
              {isCreate
                ? t(submitting ? "common.creating" : "common.create")
                : t(submitting ? "common.saving" : "common.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

interface TransferDialogProps {
  apiKey: ApiKey | null;
  onClose: () => void;
}

/** TM-UI6: user wallet → key sub-account with a before/after preview. */
function TransferDialog({ apiKey, onClose }: TransferDialogProps) {
  const { t } = useTranslation();
  const { user, refreshUser } = useAuth();
  const [amount, setAmount] = useState("");
  const [serverError, setServerError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const amountNano = usdDecimalToNano(amount);
  const amountValid = amountNano !== null && amountNano > 0n;
  const unlimited = user?.balance_unlimited ?? false;
  const accountNano = BigInt(user?.balance_nano_usd ?? "0");
  const keyNano = BigInt(apiKey?.sub_account_balance_nano_usd ?? "0");
  const exceeds = amountValid && !unlimited && amountNano > accountNano;
  const amountError =
    amount.trim() !== "" && !amountValid
      ? t("apiKeys.transferAmountInvalid")
      : exceeds
        ? t("apiKeys.transferExceedsBalance", { balance: formatNanoUsd(accountNano, 2) })
        : serverError;

  const close = () => {
    setAmount("");
    setServerError(null);
    onClose();
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!apiKey || !amountValid || exceeds) return;
    setSubmitting(true);
    setServerError(null);
    try {
      await transferToSubAccountAndRefresh(apiKey.id, amount.trim());
      await refreshUser();
      toast.success(t("apiKeys.transferSuccess"));
      close();
    } catch (error) {
      if (error instanceof DashboardApiError && error.code === "insufficient_balance") {
        setServerError(apiErrorMessage(error, t));
        await refreshUser();
      } else {
        toastApiError(error, t, t("apiKeys.transferFailed"));
      }
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Dialog open={apiKey !== null} onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent dirty={amount.trim() !== ""} className="sm:max-w-md">
        <form noValidate onSubmit={(event) => void handleSubmit(event)} className="space-y-4">
          <DialogHeader>
            <DialogTitle>{t("apiKeys.transferTitle")}</DialogTitle>
            <DialogDescription>
              {t("apiKeys.transferDescription", { name: apiKey?.name })}
            </DialogDescription>
          </DialogHeader>
          <Field data-invalid={amountError ? true : undefined}>
            <FieldLabel htmlFor="transfer-amount">{t("apiKeys.transferAmount")}</FieldLabel>
            <Input
              id="transfer-amount"
              inputMode="decimal"
              value={amount}
              aria-invalid={amountError ? true : undefined}
              onChange={(event) => {
                setAmount(event.target.value);
                setServerError(null);
              }}
              placeholder="1.00"
            />
            <FieldDescription>{t("apiKeys.transferAmountHelp")}</FieldDescription>
            {amountError && <FieldError>{amountError}</FieldError>}
          </Field>
          <p className="rounded-lg border bg-muted/30 p-3 text-sm tabular-nums">
            {unlimited
              ? t("apiKeys.transferPreviewUnlimited", {
                  keyBefore: formatNanoUsd(keyNano, 2),
                  keyAfter: formatNanoUsd(keyNano + (amountValid ? amountNano : 0n), 2),
                })
              : t("apiKeys.transferPreview", {
                  accountBefore: formatNanoUsd(accountNano, 2),
                  accountAfter: formatNanoUsd(accountNano - (amountValid ? amountNano : 0n), 2),
                  keyBefore: formatNanoUsd(keyNano, 2),
                  keyAfter: formatNanoUsd(keyNano + (amountValid ? amountNano : 0n), 2),
                })}
          </p>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={close}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={!amountValid || exceeds || submitting}>
              {t("apiKeys.transfer")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

interface AdjustBalanceDialogProps {
  apiKey: ApiKey | null;
  keys: ApiKey[];
  onClose: () => void;
  onReload: () => Promise<ApiKey[] | undefined>;
}

/** TM-UI1a / SA-ADM5: admin compare-and-set of a key's sub-account balance. */
function AdjustBalanceDialog({ apiKey, keys, onClose, onReload }: AdjustBalanceDialogProps) {
  const { t } = useTranslation();
  const { refreshUser } = useAuth();
  const [newBalance, setNewBalance] = useState("");
  // The balance this dialog showed; sent as the compare-and-set guard.
  const [expectedNano, setExpectedNano] = useState<string | null>(null);
  const [conflict, setConflict] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const currentNano = BigInt(expectedNano ?? apiKey?.sub_account_balance_nano_usd ?? "0");
  const nextNano = usdDecimalToNano(newBalance, true);
  const inputError =
    newBalance.trim() !== "" && nextNano === null ? t("apiKeys.newBalanceInvalid") : conflict;
  const unchanged = nextNano === null || nextNano === currentNano;

  const close = () => {
    setNewBalance("");
    setExpectedNano(null);
    setConflict(null);
    onClose();
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!apiKey || nextNano === null || unchanged) return;
    setSubmitting(true);
    setConflict(null);
    try {
      await updateApiKeyOptimistic(
        apiKey.id,
        {
          sub_account_balance_nano_usd: nextNano.toString(),
          expected_sub_account_balance_nano_usd: currentNano.toString(),
        },
        keys,
      );
      await refreshUser();
      toast.success(t("apiKeys.adjustSuccess"));
      close();
    } catch (error) {
      if (error instanceof DashboardApiError && error.code === "balance_changed") {
        const fresh = (await onReload())?.find((key) => key.id === apiKey.id);
        if (fresh) {
          setExpectedNano(fresh.sub_account_balance_nano_usd);
          setConflict(
            t("apiKeys.balanceChanged", {
              amount: formatUsdDecimal(fresh.sub_account_balance_usd, 2),
            }),
          );
        } else {
          toastApiError(error, t);
        }
      } else {
        toastApiError(error, t, t("apiKeys.failedUpdate"));
      }
    } finally {
      setSubmitting(false);
    }
  };

  const difference = nextNano === null ? 0n : nextNano - currentNano;

  return (
    <Dialog open={apiKey !== null} onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent dirty={newBalance.trim() !== ""} className="sm:max-w-md">
        <form noValidate onSubmit={(event) => void handleSubmit(event)} className="space-y-4">
          <DialogHeader>
            <DialogTitle>{t("apiKeys.adjustTitle")}</DialogTitle>
            <DialogDescription>{t("apiKeys.adjustDescription", { name: apiKey?.name })}</DialogDescription>
          </DialogHeader>
          <p className="text-sm">
            {t("apiKeys.currentBalance")}:{" "}
            <span className="font-medium tabular-nums">{formatNanoUsd(currentNano, 2)}</span>
          </p>
          <Field data-invalid={inputError ? true : undefined}>
            <FieldLabel htmlFor="adjust-balance">{t("apiKeys.newBalance")}</FieldLabel>
            <Input
              id="adjust-balance"
              inputMode="decimal"
              value={newBalance}
              aria-invalid={inputError ? true : undefined}
              onChange={(event) => setNewBalance(event.target.value)}
              placeholder="0.00"
            />
            {nextNano !== null && !unchanged && (
              <FieldDescription className="tabular-nums">
                {formatNanoUsd(currentNano, 2)} → {formatNanoUsd(nextNano, 2)}
                {" · "}
                {difference < 0n
                  ? t("apiKeys.adjustRefundNote", { amount: formatNanoUsd(-difference, 2) })
                  : t("apiKeys.adjustIncreaseNote", { amount: formatNanoUsd(difference, 2) })}
              </FieldDescription>
            )}
            {inputError && <FieldError>{inputError}</FieldError>}
          </Field>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={close}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={unchanged || submitting}>
              {submitting ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function ApiKeysPage() {
  const { t, i18n } = useTranslation();
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
  const [initialForm, setInitialForm] = useState<KeyForm>(EMPTY_KEY_FORM);
  const [formErrors, setFormErrors] = useState<KeyFormErrors>({});
  const [expiryDaysValid, setExpiryDaysValid] = useState(true);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [createdKey, setCreatedKey] = useState<ApiKeyCreated | null>(null);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [deleteTarget, setDeleteTarget] = useState<ApiKey | null>(null);
  const [batchDeleteOpen, setBatchDeleteOpen] = useState(false);
  const [transferKey, setTransferKey] = useState<ApiKey | null>(null);
  const [adjustKey, setAdjustKey] = useState<ApiKey | null>(null);
  const [query, setQuery] = useState("");
  const scrollParent = useContext(DashboardScrollParentContext);
  const needle = query.trim().toLowerCase();
  const visibleKeys = useMemo(
    () => keys.filter((key) => keyMatchesQuery(key, needle)),
    [keys, needle]
  );
  // AK-UI-LIST-1: optional columns follow all loaded keys, so searching never shifts the grid.
  const showSubAccountColumn = keys.some((key) => key.sub_account_enabled);
  const listColumns = tokenListColumns(showSubAccountColumn, canManageSystem);

  const openDialog = (target: KeyDialogTarget, next: KeyForm) => {
    setForm(next);
    setInitialForm(next);
    setFormErrors({});
    setExpiryDaysValid(true);
    setAdvancedOpen(false);
    setDialogTarget(target);
  };

  const openCreate = () => openDialog({ mode: "create" }, EMPTY_KEY_FORM);
  const openEdit = (key: ApiKey) => openDialog({ mode: "edit", key }, formFromKey(key));

  const showErrors = (errors: KeyFormErrors, transformIndex: number | null = null) => {
    setFormErrors(errors);
    const first = FIELD_ORDER.find((field) => errors[field]);
    if (!first) return;
    if (ADVANCED_FIELDS[first]) setAdvancedOpen(true);
    // Wait for the advanced section to mount before focusing inside it.
    requestAnimationFrame(() => {
      if (first === "transforms" && transformIndex !== null) {
        revealTransformRule("apiKey", transformIndex);
      } else {
        document.getElementById(FIELD_ELEMENT_IDS[first])?.focus();
      }
    });
  };

  const validate = (): { errors: KeyFormErrors; transformIndex: number | null } => {
    const errors: KeyFormErrors = {};
    if (!form.name.trim()) errors.name = t("apiKeys.nameRequired");
    if (dialogTarget?.mode === "create" && !expiryDaysValid) {
      errors.expiry = t("apiKeys.expiresInDaysInvalid");
    }
    if (dialogTarget?.mode === "edit" && !form.expiryNever && !form.expiryDate) {
      errors.expiry = t("apiKeys.expiryDateRequired");
    }
    if (!form.useUserGroup && form.groupIds.length === 0) {
      errors.groups = t("apiKeys.groupsRequired");
    }
    if (
      dialogTarget?.mode === "create"
      && canManageSystem
      && form.subAccountEnabled
      && usdDecimalToNano(form.initialBalanceUsd) === null
    ) {
      errors.initialBalance = t("apiKeys.initialBalanceInvalid");
    }
    if (form.maxMultiplier.trim() && normalizeMultiplier(form.maxMultiplier) == null) {
      errors.maxMultiplier = t("apiKeys.maxMultiplierInvalid");
    }
    const invalidRule = canManageSystem
      ? findFirstInvalidTransformRule(form.transforms, apiKeyTransformRegistry)
      : null;
    if (invalidRule) {
      errors.transforms = formatTransformRuleError(
        invalidRule,
        form.transforms,
        apiKeyTransformRegistry,
        t,
        i18n.language,
      );
    }
    return { errors, transformIndex: invalidRule?.index ?? null };
  };

  const handleSubmit = async () => {
    if (!dialogTarget) return;
    const { errors, transformIndex } = validate();
    if (Object.keys(errors).length > 0) {
      showErrors(errors, transformIndex);
      return;
    }
    setFormErrors({});
    setSubmitting(true);
    try {
      const maxMultiplier = form.maxMultiplier.trim()
        ? normalizeMultiplier(form.maxMultiplier) ?? undefined
        : undefined;
      const shared = {
        name: form.name.trim(),
        sub_account_enabled: form.subAccountEnabled,
        model_limits_enabled: form.modelLimitsEnabled,
        model_limits: form.modelLimits,
        ip_whitelist: form.ipWhitelist,
        use_user_group: form.useUserGroup,
        group_ids: form.useUserGroup ? [] : form.groupIds,
        max_multiplier: maxMultiplier,
        model_redirects: form.modelRedirects.filter(
          (rule) => rule.pattern.trim() && rule.replace.trim()
        ),
        reasoning_envelope_enabled: form.reasoningEnvelopeEnabled,
        request_capture_mode: form.requestCaptureMode,
        request_capture_retention: form.requestCaptureRetention,
        // Non-admins never see or edit transforms; leave the stored chain untouched.
        ...(canManageSystem ? { transforms: form.transforms } : {}),
      };
      if (dialogTarget.mode === "create") {
        const initialBalance = usdDecimalToNano(form.initialBalanceUsd);
        const input: CreateApiKeyInput = {
          ...shared,
          expires_in_days: form.expiresInDays ?? undefined,
          ...(canManageSystem && form.subAccountEnabled && initialBalance !== null
            ? { sub_account_balance_nano_usd: initialBalance.toString() }
            : {}),
        };
        setCreatedKey(await createApiKeyOptimistic(input, keys));
      } else {
        const expiryChanged =
          form.expiryNever !== initialForm.expiryNever
          || (!form.expiryNever && form.expiryDate !== initialForm.expiryDate);
        // TM-UI1: the edit form never carries a sub-account balance.
        const input: UpdateApiKeyInput = {
          ...shared,
          ...(expiryChanged
            ? { expires_at: form.expiryNever ? null : endOfLocalDayIso(form.expiryDate) }
            : {}),
        };
        await updateApiKeyOptimistic(dialogTarget.key.id, input, keys);
        toast.success(t("apiKeys.updateSuccess"));
      }
      setDialogTarget(null);
    } catch (error) {
      const param = apiErrorParam(error);
      if (param === "expires_in_days" || param === "expires_at") {
        showErrors({ expiry: t("apiKeys.expiresInDaysInvalid") });
      } else {
        toastApiError(
          error,
          t,
          t(dialogTarget.mode === "create" ? "apiKeys.failedCreate" : "apiKeys.failedUpdate"),
        );
      }
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
      toastApiError(error, t, t("apiKeys.failedDelete"));
    }
  };

  const confirmBatchDelete = async () => {
    try {
      await batchDeleteApiKeysOptimistic(selectedKeys, keys);
      setSelectedKeys([]);
    } catch (error) {
      toastApiError(error, t, t("apiKeys.failedBatchDelete"));
    }
  };

  const handleToggleEnabled = async (key: ApiKey) => {
    try {
      await updateApiKeyOptimistic(key.id, { enabled: !key.enabled }, keys);
    } catch (error) {
      toastApiError(error, t, t("apiKeys.failedUpdate"));
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

      <ApiBaseUrlRow />

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
            <CardContent className="space-y-2">
              <div className="flex items-center gap-2">
                <code className="min-w-0 flex-1 break-all rounded-lg border bg-muted px-3 py-2 text-sm">
                  {createdKey.key}
                </code>
                <CopyButton value={createdKey.key} />
              </div>
              <ApiBaseUrlRow />
              <Button variant="ghost" size="sm" onClick={() => setCreatedKey(null)}>
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
            <DataList columns={listColumns} dense>
              <DataListHeader>
                <DataListHead className="pl-7">{t("common.name")}</DataListHead>
                <DataListHead>{t("apiKeys.keyPrefix")}</DataListHead>
                <DataListHead>{t("apiKeys.restrictions")}</DataListHead>
                <DataListHead>{t("apiKeys.expires")}</DataListHead>
                {showSubAccountColumn && (
                  <DataListHead align="end">{t("apiKeys.subAccountEnabled")}</DataListHead>
                )}
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
                    const keyPrefix = (
                      <span className="inline-flex min-w-0 max-w-full items-center gap-1 align-middle">
                        <code className="truncate text-muted-foreground">
                          {displayedKeyPrefix(key)}…
                        </code>
                        {key.key && <CopyButton value={key.key} />}
                      </span>
                    );
                    const expiry = key.expires_at ? (
                      <span className="inline-flex items-center gap-2 whitespace-nowrap">
                        <span className={cn("tabular-nums", expired && "text-muted-foreground")}>
                          {formatDate(key.expires_at)}
                        </span>
                        {expired && <StatusBadge variant="warning">{t("apiKeys.expired")}</StatusBadge>}
                      </span>
                    ) : (
                      <span className="whitespace-nowrap text-muted-foreground">{t("apiKeys.noExpiry")}</span>
                    );
                    const subAccountBalance = (
                      <span className="tabular-nums">{formatUsdDecimal(key.sub_account_balance_usd, 2)}</span>
                    );
                    const enabledSwitch = (
                      <Switch
                        className="align-middle"
                        checked={key.enabled}
                        aria-label={t("common.enableItem", { name: key.name })}
                        onCheckedChange={() => handleToggleEnabled(key)}
                      />
                    );
                    const rowActions = (
                      <>
                        {key.sub_account_enabled && (
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-11 touch-manipulation sm:size-9"
                            title={t("apiKeys.transferBalance")}
                            aria-label={t("apiKeys.transferBalanceItem", { name: key.name })}
                            onClick={() => setTransferKey(key)}
                          >
                            <ArrowRightLeft />
                          </Button>
                        )}
                        {key.sub_account_enabled && canManageSystem && (
                          <Button
                            variant="ghost"
                            size="icon"
                            className="size-11 touch-manipulation sm:size-9"
                            title={t("apiKeys.adjustBalance")}
                            aria-label={t("apiKeys.adjustBalanceItem", { name: key.name })}
                            onClick={() => setAdjustKey(key)}
                          >
                            <Coins />
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
                      </>
                    );
                    return (
                      <>
                        <DataListCell primary>
                          <div className="flex min-w-0 items-center gap-3">
                            <Checkbox
                              checked={selectedKeys.includes(key.id)}
                              aria-label={key.name}
                              onCheckedChange={() => toggleSelectKey(key.id)}
                            />
                            {/* AK11: the group badges yield to the name when the name cell is narrow. */}
                            <div className="flex min-w-0 flex-1 items-center gap-3 [container-type:inline-size]">
                              <span
                                className={cn(
                                  "min-w-0 truncate font-medium",
                                  (!key.enabled || expired) && "text-muted-foreground"
                                )}
                                title={key.name}
                              >
                                {key.name}
                              </span>
                              {!key.use_user_group && key.group_ids.length > 0 && (
                                <span className="hidden shrink-0 [@container(min-width:12rem)]:block">
                                  <GroupsBadge groupIds={key.group_ids} variant="secondary" />
                                </span>
                              )}
                            </div>
                            <span className={cn("shrink-0", NARROW_ONLY)}>{enabledSwitch}</span>
                          </div>
                        </DataListCell>
                        {/* AK-UI-LIST-2: below wide mode one metadata group replaces the labelled cells; the actions share its line while it keeps 14rem. */}
                        <DataListCell className={cn("col-span-full flex flex-wrap items-center gap-x-4 gap-y-1", NARROW_ONLY)}>
                          <div className="flex min-w-0 flex-[1_1_14rem] flex-wrap items-center gap-x-4 gap-y-1">
                            {keyPrefix}
                            <ApiKeyRestrictionBadges apiKey={key} t={t} empty={null} />
                            {expiry}
                            {key.sub_account_enabled && (
                              <span className="whitespace-nowrap">
                                <span className="text-muted-foreground">{t("apiKeys.subAccountEnabled")} </span>
                                {subAccountBalance}
                              </span>
                            )}
                          </div>
                          <div className="ml-auto flex shrink-0 items-center gap-1">{rowActions}</div>
                        </DataListCell>
                        <DataListCell label={t("apiKeys.keyPrefix")} className={WIDE_ONLY}>
                          {keyPrefix}
                        </DataListCell>
                        <DataListCell label={t("apiKeys.restrictions")} className={WIDE_ONLY}>
                          <ApiKeyRestrictionBadges apiKey={key} t={t} />
                        </DataListCell>
                        <DataListCell label={t("apiKeys.expires")} className={WIDE_ONLY}>
                          {expiry}
                        </DataListCell>
                        {showSubAccountColumn && (
                          <DataListCell label={t("apiKeys.subAccountEnabled")} align="end" className={WIDE_ONLY}>
                            {key.sub_account_enabled ? subAccountBalance : <span className="text-muted-foreground">—</span>}
                          </DataListCell>
                        )}
                        <DataListCell label={t("common.status")} className={WIDE_ONLY}>
                          {enabledSwitch}
                        </DataListCell>
                        <DataListActions className="hidden [@container(min-width:56rem)]:flex">
                          {rowActions}
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
        initialForm={initialForm}
        errors={formErrors}
        expiryDaysValid={expiryDaysValid}
        advancedOpen={advancedOpen}
        onAdvancedOpenChange={setAdvancedOpen}
        onExpiryDaysValidityChange={setExpiryDaysValid}
        onFormChange={(patch) => {
          setForm((prev) => ({ ...prev, ...patch }));
          // Editing a field clears its error; the remaining errors stay until the next submit.
          setFormErrors((prev) => {
            const next = { ...prev };
            if ("name" in patch) delete next.name;
            if ("expiryDate" in patch || "expiryNever" in patch || "expiresInDays" in patch) delete next.expiry;
            if ("initialBalanceUsd" in patch) delete next.initialBalance;
            if ("groupIds" in patch || "useUserGroup" in patch) delete next.groups;
            if ("maxMultiplier" in patch) delete next.maxMultiplier;
            if ("transforms" in patch) delete next.transforms;
            return next;
          });
        }}
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

      <TransferDialog apiKey={transferKey} onClose={() => setTransferKey(null)} />

      <AdjustBalanceDialog
        apiKey={adjustKey}
        keys={keys}
        onClose={() => setAdjustKey(null)}
        onReload={() => mutate()}
      />
    </PageWrapper>
  );
}
