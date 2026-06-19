import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Copy, Check, Key, Edit, Globe, Layers, Settings2, ArrowRightLeft, X } from "lucide-react";
import { BadgeOverflowList } from "@/components/BadgeOverflowList";
import { GroupsBadge } from "@/components/GroupsBadge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
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
import { TableVirtuoso } from "react-virtuoso";
import {
  useApiKeys,
  createApiKeyOptimistic,
  updateApiKeyOptimistic,
  deleteApiKeyOptimistic,
  batchDeleteApiKeysOptimistic,
  useDashboardGroups,
  useTransformRegistry,
} from "@/lib/swr";
import type { ApiKey, ApiKeyCreated, CreateApiKeyInput, ModelRedirectRule, RequestCaptureMode, TransformRuleConfig, UpdateApiKeyInput } from "@/lib/api";
import { api as apiClient } from "@/lib/api";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { TablePageSkeleton } from "@/components/ui/page-skeleton";
import { DataTableShell, VirtualTableCell, VirtualTableHeaderCell } from "@/components/ui/data-table-shell";
import { EmptyState } from "@/components/ui/empty-state";
import { TransformChainEditor } from "@/components/transforms/transform-chain-editor";
import { findFirstInvalidTransformRule } from "@/components/transforms/transform-schema";
import { toast } from "sonner";
import { useAuth } from "@/hooks/use-auth";

function groupKey(value: string): string {
  return value.trim().toLowerCase();
}

function dedupeAllowedGroups(values: string[]): string[] {
  const seen = new Set<string>();
  const next: string[] = [];

  for (const value of values) {
    const trimmed = value.trim();
    const key = groupKey(trimmed);
    if (!key || seen.has(key)) {
      continue;
    }
    seen.add(key);
    next.push(trimmed);
  }

  return next;
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
    return <span className="text-muted-foreground">-</span>;
  }

  return (
    <BadgeOverflowList
      items={items}
      visibleCount={2}
      ariaLabel={t("apiKeys.restrictions")}
      contentClassName="max-w-[min(34rem,calc(100vw-2rem))]"
    />
  );
}

interface AllowedGroupsInputProps {
  inputId: string;
  value: string[];
  suggestions: string[];
  suggestionsLoading: boolean;
  currentUserAllowedGroups: string[] | null;
  onChange: (next: string[]) => void;
}

function AllowedGroupsInput({
  inputId,
  value,
  suggestions,
  suggestionsLoading,
  currentUserAllowedGroups,
  onChange,
}: AllowedGroupsInputProps) {
  const { t } = useTranslation();
  const [draft, setDraft] = useState("");
  const groups = useMemo(() => dedupeAllowedGroups(value), [value]);
  const groupsRef = useRef(groups);
  const currentUserGroups = useMemo(
    () => dedupeAllowedGroups(currentUserAllowedGroups ?? []),
    [currentUserAllowedGroups]
  );
  const draftKey = groupKey(draft);

  useEffect(() => {
    groupsRef.current = groups;
  }, [groups]);

  const filteredSuggestions = useMemo(
    () =>
      suggestions.filter((suggestion) => {
        const suggestionKey = groupKey(suggestion);
        if (!suggestionKey) {
          return false;
        }
        if (groups.some((group) => groupKey(group) === suggestionKey)) {
          return false;
        }
        return !draftKey || suggestionKey.includes(draftKey);
      }),
    [draftKey, groups, suggestions]
  );

  const commitGroups = (nextValues: string[]) => {
    const nextGroups = dedupeAllowedGroups(nextValues);
    groupsRef.current = nextGroups;
    onChange(nextGroups);
  };

  const flushDraft = () => {
    const parts = draft
      .split(",")
      .map((part) => part.trim())
      .filter(Boolean);

    if (parts.length > 0) {
      commitGroups([...groupsRef.current, ...parts]);
    }

    setDraft("");
  };

  const removeGroup = (group: string) => {
    commitGroups(groupsRef.current.filter((entry) => groupKey(entry) !== groupKey(group)));
  };

  const addSuggestion = (group: string) => {
    commitGroups([...groupsRef.current, group]);
    setDraft("");
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={inputId}>{t("apiKeys.allowedGroups")}</Label>
        <span className="text-xs text-muted-foreground">{t("providers.optional")}</span>
      </div>
      <Input
        id={inputId}
        value={draft}
        placeholder={t("providers.groupsPlaceholder")}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={flushDraft}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            flushDraft();
          }
        }}
      />
      <p className="text-xs text-muted-foreground">
        {groups.length === 0
          ? t("apiKeys.allowedGroupsEmptyHelp")
          : t("apiKeys.allowedGroupsSelectedHelp")}
      </p>
      {groups.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {groups.map((group) => (
            <Badge
              key={groupKey(group)}
              variant="secondary"
              className="flex max-w-full items-center gap-1 font-mono"
            >
              <span className="min-w-0 truncate">{group}</span>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-4 w-4 shrink-0"
                onClick={() => removeGroup(group)}
              >
                <X className="h-3 w-3" />
              </Button>
            </Badge>
          ))}
        </div>
      )}
      {suggestionsLoading ? (
        <div className="flex flex-wrap gap-2">
          <Skeleton className="h-7 w-20 rounded-full" />
          <Skeleton className="h-7 w-24 rounded-full" />
          <Skeleton className="h-7 w-16 rounded-full" />
        </div>
      ) : filteredSuggestions.length > 0 ? (
        <div className="flex flex-wrap gap-2">
          {filteredSuggestions.slice(0, 8).map((group) => (
            <Button
              key={group}
              type="button"
              variant="outline"
              size="sm"
              className="h-7 rounded-full px-3 font-mono text-xs"
              onClick={() => addSuggestion(group)}
            >
              {group}
            </Button>
          ))}
        </div>
      ) : null}
      {currentUserAllowedGroups !== null && (
        currentUserGroups.length > 0 ? (
          <div className="space-y-2">
            <p className="text-xs text-muted-foreground">
              {t("apiKeys.allowedGroupsCurrentUserHint")}
            </p>
            <GroupsBadge groups={currentUserGroups} />
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">
            {t("apiKeys.allowedGroupsCurrentUserAllHint")}
          </p>
        )
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
            className="h-8 w-8 shrink-0"
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
  const { data: groupSuggestions = [], isLoading: groupsLoading } = useDashboardGroups();
  const { data: transformRegistry = [] } = useTransformRegistry();
  const apiKeyTransformRegistry = useMemo(
    () => transformRegistry.filter((item) => item.supported_scopes.includes("api_key")),
    [transformRegistry]
  );
  const [createOpen, setCreateOpen] = useState(false);
  const [editKey, setEditKey] = useState<ApiKey | null>(null);
  const [selectedKeys, setSelectedKeys] = useState<string[]>([]);
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null);
  const [batchDeleteOpen, setBatchDeleteOpen] = useState(false);

  // Create form state
  const [newKeyName, setNewKeyName] = useState("");
  const [newKeyExpires, setNewKeyExpires] = useState("");
  const [newKeySubAccountEnabled, setNewKeySubAccountEnabled] = useState(false);
  const [transferDialogKey, setTransferDialogKey] = useState<ApiKey | null>(null);
  const [transferAmount, setTransferAmount] = useState("");
  const [transferring, setTransferring] = useState(false);
  const [newKeyModelLimitsEnabled, setNewKeyModelLimitsEnabled] = useState(false);
  const [newKeyModelLimits, setNewKeyModelLimits] = useState("");
  const [newKeyIpWhitelist, setNewKeyIpWhitelist] = useState("");

  const [newKeyAllowedGroups, setNewKeyAllowedGroups] = useState<string[]>([]);
  const [newKeyMaxMultiplier, setNewKeyMaxMultiplier] = useState("");
  const [newKeyTransforms, setNewKeyTransforms] = useState<TransformRuleConfig[]>([]);
  const [newKeyModelRedirects, setNewKeyModelRedirects] = useState<ModelRedirectRule[]>([]);
  const [newKeyReasoningEnvelopeEnabled, setNewKeyReasoningEnvelopeEnabled] = useState(true);
  const [newKeyRequestCaptureMode, setNewKeyRequestCaptureMode] = useState<RequestCaptureMode>("off");

  const [creating, setCreating] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [createdKey, setCreatedKey] = useState<ApiKeyCreated | null>(null);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const resetCreateForm = () => {
    setNewKeyName("");
    setNewKeyExpires("");
    setNewKeySubAccountEnabled(false);
    setNewKeyModelLimitsEnabled(false);
    setNewKeyModelLimits("");
    setNewKeyIpWhitelist("");

    setNewKeyAllowedGroups([]);
    setNewKeyMaxMultiplier("");
    setNewKeyTransforms([]);
    setNewKeyModelRedirects([]);
    setNewKeyReasoningEnvelopeEnabled(true);
    setNewKeyRequestCaptureMode("off");
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
    setCreating(true);
    try {
      const input: CreateApiKeyInput = {
        name: newKeyName.trim(),
        expires_in_days: newKeyExpires ? parseInt(newKeyExpires) : undefined,
        sub_account_enabled: newKeySubAccountEnabled,
        model_limits_enabled: newKeyModelLimitsEnabled,
        model_limits: newKeyModelLimits ? newKeyModelLimits.split(",").map(s => s.trim()).filter(s => s) : [],
        ip_whitelist: newKeyIpWhitelist ? newKeyIpWhitelist.split(",").map(s => s.trim()).filter(s => s) : [],
        allowed_groups: dedupeAllowedGroups(newKeyAllowedGroups),
        max_multiplier: newKeyMaxMultiplier ? parseFloat(newKeyMaxMultiplier) : undefined,
        transforms: newKeyTransforms,
        model_redirects: newKeyModelRedirects.filter((r) => r.pattern.trim() && r.replace.trim()),
        reasoning_envelope_enabled: newKeyReasoningEnvelopeEnabled,
        request_capture_mode: newKeyRequestCaptureMode,
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
    setUpdating(true);
    try {
      const input: UpdateApiKeyInput = {
        name: newKeyName.trim() || undefined,
        sub_account_enabled: newKeySubAccountEnabled,
        model_limits_enabled: newKeyModelLimitsEnabled,
        model_limits: newKeyModelLimits ? newKeyModelLimits.split(",").map(s => s.trim()).filter(s => s) : [],
        ip_whitelist: newKeyIpWhitelist ? newKeyIpWhitelist.split(",").map(s => s.trim()).filter(s => s) : [],
        allowed_groups: dedupeAllowedGroups(newKeyAllowedGroups),
        max_multiplier: newKeyMaxMultiplier ? parseFloat(newKeyMaxMultiplier) : undefined,
        transforms: newKeyTransforms,
        model_redirects: newKeyModelRedirects.filter((r) => r.pattern.trim() && r.replace.trim()),
        reasoning_envelope_enabled: newKeyReasoningEnvelopeEnabled,
        request_capture_mode: newKeyRequestCaptureMode,
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
    setNewKeyModelLimitsEnabled(key.model_limits_enabled);
    setNewKeyModelLimits(key.model_limits.join(", "));
    setNewKeyIpWhitelist(key.ip_whitelist.join(", "));
    setNewKeyAllowedGroups(key.allowed_groups ?? []);
    setNewKeyMaxMultiplier(key.max_multiplier != null ? String(key.max_multiplier) : "");
    setNewKeyTransforms(key.transforms ?? []);
    setNewKeyModelRedirects(key.model_redirects ?? []);
    setNewKeyReasoningEnvelopeEnabled(key.reasoning_envelope_enabled ?? true);
    setNewKeyRequestCaptureMode(key.request_capture_mode ?? "off");
  };

  const toggleSelectKey = (id: string) => {
    setSelectedKeys(prev =>
      prev.includes(id) ? prev.filter(k => k !== id) : [...prev, id]
    );
  };

  const toggleSelectAll = () => {
    if (selectedKeys.length === keys.length) {
      setSelectedKeys([]);
    } else {
      setSelectedKeys(keys.map(k => k.id));
    }
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
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader title={t("apiKeys.title")} description={t("apiKeys.description")} actions={(
          <>
          {selectedKeys.length > 0 && (
            <Button variant="destructive" onClick={handleBatchDelete}>
              <Trash2 className="mr-2 h-4 w-4" />
              {t("apiKeys.deleteSelected", { count: selectedKeys.length })}
            </Button>
          )}
          <Dialog open={createOpen} onOpenChange={setCreateOpen}>
            <DialogTrigger asChild>
              <motion.div whileHover={{ scale: 1.02 }} whileTap={{ scale: 0.98 }}>
                <Button>
                  <Plus className="mr-2 h-4 w-4" />
                  {t("apiKeys.createKey")}
                </Button>
              </motion.div>
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
                <AllowedGroupsInput
                  inputId="allowedGroups"
                  value={newKeyAllowedGroups}
                  suggestions={groupSuggestions}
                  suggestionsLoading={groupsLoading}
                  currentUserAllowedGroups={currentUser?.allowed_groups ?? null}
                  onChange={(allowedGroups) => setNewKeyAllowedGroups(allowedGroups)}
                />
                <div className="flex items-center space-x-2">
                  <Switch
                    id="subAccountEnabled"
                    checked={newKeySubAccountEnabled}
                    onCheckedChange={setNewKeySubAccountEnabled}
                  />
                  <Label htmlFor="subAccountEnabled">{t("apiKeys.subAccountEnabled")}</Label>
                </div>
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
                    type="number"
                    min="0"
                    step="0.1"
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
      </motion.div>

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
                <motion.div whileHover={{ scale: 1.1 }} whileTap={{ scale: 0.9 }}>
                  <Button
                    variant="outline"
                    size="icon"
                    onClick={() => handleCopy(createdKey.key)}
                  >
                    {copiedKey === createdKey.key ? (
                      <Check className="h-4 w-4" />
                    ) : (
                      <Copy className="h-4 w-4" />
                    )}
                  </Button>
                </motion.div>
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

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        <DataTableShell
          toolbar={(
            <div>
              <h2 className="text-base font-semibold">{t("apiKeys.yourApiKeys")}</h2>
              <p className="text-sm text-muted-foreground">
                {t("apiKeys.keysCreated", { count: keys.length })}
              </p>
            </div>
          )}
          isEmpty={keys.length === 0}
          emptyState={(
            <EmptyState
              icon={<Key className="h-12 w-12" />}
              title={t("apiKeys.yourApiKeys")}
              description={t("apiKeys.noKeys")}
            />
          )}
        >
              <TableVirtuoso
                style={{ height: "calc(100dvh - 280px)", minHeight: 400, overflowX: "auto" }}
                data={keys}
                components={{
                  Table: (props) => (
                    <table
                      {...props}
                      className="w-full caption-bottom text-sm"
                      style={{ minWidth: "72rem" }}
                    />
                  ),
                  TableHead: (props) => (
                    <thead {...props} className="[&_tr]:border-b" />
                  ),
                  TableRow: (props) => (
                    <tr
                      {...props}
                      className="border-b transition-colors hover:bg-muted/50"
                    />
                  ),
                  TableBody: (props) => (
                    <tbody {...props} className="[&_tr:last-child]:border-0" />
                  ),
                }}
                fixedHeaderContent={() => (
                  <tr className="border-b bg-background">
                    <VirtualTableHeaderCell className="w-[50px] whitespace-nowrap">
                      <Checkbox
                        checked={selectedKeys.length === keys.length && keys.length > 0}
                        onCheckedChange={toggleSelectAll}
                      />
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="whitespace-nowrap">
                      {t("common.name")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="whitespace-nowrap">
                      {t("apiKeys.keyPrefix")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="whitespace-nowrap">
                      {t("apiKeys.balance")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="whitespace-nowrap">
                      {t("apiKeys.restrictions")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="whitespace-nowrap">
                      {t("apiKeys.expires")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="whitespace-nowrap">
                      {t("common.status")}
                    </VirtualTableHeaderCell>
                    <VirtualTableHeaderCell className="w-[100px] whitespace-nowrap">
                      {t("common.actions")}
                    </VirtualTableHeaderCell>
                  </tr>
                )}
                itemContent={(_index, key) => (
                  <>
                    <VirtualTableCell>
                      <Checkbox
                        checked={selectedKeys.includes(key.id)}
                        onCheckedChange={() => toggleSelectKey(key.id)}
                      />
                    </VirtualTableCell>
                    <VirtualTableCell className="font-medium whitespace-nowrap">
                      <div className="min-w-max">
                        {key.name}
                      </div>
                      {key.allowed_groups && key.allowed_groups.length > 0 && (
                        <div className="mt-1 min-w-max">
                          <GroupsBadge groups={key.allowed_groups} variant="secondary" />
                        </div>
                      )}
                    </VirtualTableCell>
                    <VirtualTableCell className="whitespace-nowrap">
                      <div className="flex min-w-max items-center gap-1">
                        <code className="rounded bg-muted px-2 py-0.5 text-sm whitespace-nowrap">
                          {key.key ? `${key.key.slice(0, 12)}...` : `${key.key_prefix}...`}
                        </code>
                        {key.key && (
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-6 w-6"
                            onClick={() => handleCopy(key.key)}
                          >
                            {copiedKey === key.key ? (
                              <Check className="h-3 w-3" />
                            ) : (
                              <Copy className="h-3 w-3" />
                            )}
                          </Button>
                        )}
                      </div>
                    </VirtualTableCell>
                    <VirtualTableCell className="whitespace-nowrap">
                      {key.sub_account_enabled ? (
                        <span className="font-mono text-sm">${key.sub_account_balance_usd}</span>
                      ) : (
                        <Badge variant="secondary" className="whitespace-nowrap">{t("apiKeys.inheritsUser")}</Badge>
                      )}
                    </VirtualTableCell>
                    <VirtualTableCell className="whitespace-nowrap">
                      <ApiKeyRestrictionBadges apiKey={key} t={t} />
                    </VirtualTableCell>
                    <VirtualTableCell className="whitespace-nowrap">
                      {key.expires_at ? formatDate(key.expires_at) : t("common.never")}
                    </VirtualTableCell>
                    <VirtualTableCell className="whitespace-nowrap">
                      <Switch
                        checked={key.enabled}
                        onCheckedChange={() => handleToggleEnabled(key)}
                      />
                    </VirtualTableCell>
                    <VirtualTableCell>
                      <div className="flex gap-1">
                        {key.sub_account_enabled && (
                          <Button
                            variant="ghost"
                            size="icon"
                            onClick={() => {
                              setTransferDialogKey(key);
                              setTransferAmount("");
                            }}
                          >
                            <ArrowRightLeft className="h-4 w-4" />
                          </Button>
                        )}
                        <Button
                          variant="ghost"
                          size="icon"
                          onClick={() => openEditDialog(key)}
                        >
                          <Edit className="h-4 w-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          onClick={() => handleDelete(key.id)}
                          className="text-destructive hover:text-destructive"
                        >
                          <Trash2 className="h-4 w-4" />
                        </Button>
                      </div>
                    </VirtualTableCell>
                  </>
                )}
              />
        </DataTableShell>
      </motion.div>

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
            <AllowedGroupsInput
              inputId="editAllowedGroups"
              value={newKeyAllowedGroups}
              suggestions={groupSuggestions}
              suggestionsLoading={groupsLoading}
              currentUserAllowedGroups={currentUser?.allowed_groups ?? null}
              onChange={(allowedGroups) => setNewKeyAllowedGroups(allowedGroups)}
            />
            <div className="flex items-center space-x-2">
              <Switch
                id="editSubAccountEnabled"
                checked={newKeySubAccountEnabled}
                onCheckedChange={setNewKeySubAccountEnabled}
              />
              <Label htmlFor="editSubAccountEnabled">{t("apiKeys.subAccountEnabled")}</Label>
            </div>
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
                type="number"
                min="0"
                step="0.1"
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
