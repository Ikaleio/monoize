import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Trash2, Copy, Check, Key, Edit, Globe, Layers, Settings2, X } from "lucide-react";
import { GroupsBadge } from "@/components/GroupsBadge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { TableVirtuoso } from "react-virtuoso";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import {
  useApiKeys,
  createApiKeyOptimistic,
  updateApiKeyOptimistic,
  deleteApiKeyOptimistic,
  batchDeleteApiKeysOptimistic,
  useDashboardGroups,
  useTransformRegistry,
} from "@/lib/swr";
import type { ApiKey, ApiKeyCreated, CreateApiKeyInput, TransformRuleConfig, UpdateApiKeyInput } from "@/lib/api";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
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

function logHandledOptimisticError(action: string, error: unknown, details: Record<string, unknown>) {
  console.debug(`[api-keys] ${action} failed after optimistic helper handling`, {
    ...details,
    error,
  });
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
              className="flex items-center gap-1 font-mono"
            >
              <span>{group}</span>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                className="h-4 w-4"
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
            <div className="flex flex-wrap gap-2">
              {currentUserGroups.map((group) => (
                <Badge
                  key={`hint-${groupKey(group)}`}
                  variant="outline"
                  className="font-mono text-xs"
                >
                  {group}
                </Badge>
              ))}
            </div>
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

  // Create form state
  const [newKeyName, setNewKeyName] = useState("");
  const [newKeyExpires, setNewKeyExpires] = useState("");
  const [newKeyQuota, setNewKeyQuota] = useState("");
  const [newKeyQuotaUnlimited, setNewKeyQuotaUnlimited] = useState(true);
  const [newKeyModelLimitsEnabled, setNewKeyModelLimitsEnabled] = useState(false);
  const [newKeyModelLimits, setNewKeyModelLimits] = useState("");
  const [newKeyIpWhitelist, setNewKeyIpWhitelist] = useState("");
  const [newKeyGroup, setNewKeyGroup] = useState("default");
  const [newKeyAllowedGroups, setNewKeyAllowedGroups] = useState<string[]>([]);
  const [newKeyMaxMultiplier, setNewKeyMaxMultiplier] = useState("");
  const [newKeyTransforms, setNewKeyTransforms] = useState<TransformRuleConfig[]>([]);

  const [creating, setCreating] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [createdKey, setCreatedKey] = useState<ApiKeyCreated | null>(null);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const resetCreateForm = () => {
    setNewKeyName("");
    setNewKeyExpires("");
    setNewKeyQuota("");
    setNewKeyQuotaUnlimited(true);
    setNewKeyModelLimitsEnabled(false);
    setNewKeyModelLimits("");
    setNewKeyIpWhitelist("");
    setNewKeyGroup("default");
    setNewKeyAllowedGroups([]);
    setNewKeyMaxMultiplier("");
    setNewKeyTransforms([]);
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
        quota: newKeyQuota ? parseInt(newKeyQuota) : undefined,
        quota_unlimited: newKeyQuotaUnlimited,
        model_limits_enabled: newKeyModelLimitsEnabled,
        model_limits: newKeyModelLimits ? newKeyModelLimits.split(",").map(s => s.trim()).filter(s => s) : [],
        ip_whitelist: newKeyIpWhitelist ? newKeyIpWhitelist.split(",").map(s => s.trim()).filter(s => s) : [],
        group: newKeyGroup || "default",
        allowed_groups: dedupeAllowedGroups(newKeyAllowedGroups),
        max_multiplier: newKeyMaxMultiplier ? parseFloat(newKeyMaxMultiplier) : undefined,
        transforms: newKeyTransforms,
      };
      const key = await createApiKeyOptimistic(
        input,
        keys,
        (error) => console.error(t("apiKeys.failedCreate"), error)
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
        quota: newKeyQuota ? parseInt(newKeyQuota) : undefined,
        quota_unlimited: newKeyQuotaUnlimited,
        model_limits_enabled: newKeyModelLimitsEnabled,
        model_limits: newKeyModelLimits ? newKeyModelLimits.split(",").map(s => s.trim()).filter(s => s) : [],
        ip_whitelist: newKeyIpWhitelist ? newKeyIpWhitelist.split(",").map(s => s.trim()).filter(s => s) : [],
        group: newKeyGroup || "default",
        allowed_groups: dedupeAllowedGroups(newKeyAllowedGroups),
        max_multiplier: newKeyMaxMultiplier ? parseFloat(newKeyMaxMultiplier) : undefined,
        transforms: newKeyTransforms,
      };
      await updateApiKeyOptimistic(
        editKey.id,
        input,
        keys,
        (error) => console.error(t("apiKeys.failedUpdate"), error)
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
    if (!confirm(t("apiKeys.confirmDelete"))) return;
    try {
      await deleteApiKeyOptimistic(
        id,
        keys,
        (error) => console.error(t("apiKeys.failedDelete"), error)
      );
      setSelectedKeys(prev => prev.filter(k => k !== id));
    } catch (error) {
      logHandledOptimisticError("delete api key", error, { id });
    }
  };

  const handleBatchDelete = async () => {
    if (selectedKeys.length === 0) return;
    if (!confirm(t("apiKeys.confirmBatchDelete", { count: selectedKeys.length }))) return;
    try {
      await batchDeleteApiKeysOptimistic(
        selectedKeys,
        keys,
        (error) => console.error(t("apiKeys.failedBatchDelete"), error)
      );
      setSelectedKeys([]);
    } catch (error) {
      logHandledOptimisticError("batch delete api keys", error, {
        count: selectedKeys.length,
        ids: selectedKeys,
      });
    }
  };

  const handleToggleEnabled = async (key: ApiKey) => {
    try {
      await updateApiKeyOptimistic(
        key.id,
        { enabled: !key.enabled },
        keys,
        (error) => console.error(t("apiKeys.failedUpdate"), error)
      );
    } catch (error) {
      logHandledOptimisticError("toggle api key enabled", error, {
        id: key.id,
        nextEnabled: !key.enabled,
      });
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
    setNewKeyQuota(key.quota_remaining?.toString() || "");
    setNewKeyQuotaUnlimited(key.quota_unlimited);
    setNewKeyModelLimitsEnabled(key.model_limits_enabled);
    setNewKeyModelLimits(key.model_limits.join(", "));
    setNewKeyIpWhitelist(key.ip_whitelist.join(", "));
    setNewKeyGroup(key.group);
    setNewKeyAllowedGroups(key.allowed_groups ?? []);
    setNewKeyMaxMultiplier(key.max_multiplier != null ? String(key.max_multiplier) : "");
    setNewKeyTransforms(key.transforms ?? []);
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
      <div className="space-y-6">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-64" />
      </div>
    );
  }

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
        className="flex items-center justify-between"
      >
        <div>
          <h1 className="text-3xl font-bold tracking-tight">{t("apiKeys.title")}</h1>
          <p className="text-muted-foreground">
            {t("apiKeys.description")}
          </p>
        </div>
        <div className="flex gap-2">
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
            <DialogContent className="max-w-4xl max-h-[85vh] overflow-y-auto">
              <DialogHeader>
                <DialogTitle>{t("apiKeys.createApiKey")}</DialogTitle>
                <DialogDescription>
                  {t("apiKeys.createDescription")}
                </DialogDescription>
              </DialogHeader>
              <div className="space-y-4 py-4 max-h-[60vh] overflow-y-auto px-1 -mx-1">
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
                <div className="space-y-2">
                  <Label htmlFor="group">{t("apiKeys.group")}</Label>
                  <Input
                    id="group"
                    value={newKeyGroup}
                    onChange={(e) => setNewKeyGroup(e.target.value)}
                    placeholder="default"
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
                    id="quotaUnlimited"
                    checked={newKeyQuotaUnlimited}
                    onCheckedChange={setNewKeyQuotaUnlimited}
                  />
                  <Label htmlFor="quotaUnlimited">{t("apiKeys.unlimitedQuota")}</Label>
                </div>
                {!newKeyQuotaUnlimited && (
                  <div className="space-y-2">
                    <Label htmlFor="quota">{t("apiKeys.quotaRemaining")}</Label>
                    <Input
                      id="quota"
                      type="number"
                      min="0"
                      value={newKeyQuota}
                      onChange={(e) => setNewKeyQuota(e.target.value)}
                      placeholder="1000"
                    />
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
              </div>
              <DialogFooter>
                <Button variant="outline" onClick={() => { setCreateOpen(false); resetCreateForm(); }}>
                  {t("common.cancel")}
                </Button>
                <Button onClick={handleCreate} disabled={creating || !newKeyName.trim()}>
                  {creating ? t("common.creating") : t("common.create")}
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>
        </div>
      </motion.div>

      {createdKey && (
        <motion.div
          initial={{ opacity: 0, y: 20, scale: 0.95 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          transition={{ type: "spring", stiffness: 300, damping: 25 }}
        >
          <Card className="border-green-500/50 bg-green-500/5">
            <CardHeader>
              <CardTitle className="flex items-center gap-2 text-green-600">
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
        <Card>
          <CardHeader>
            <CardTitle>{t("apiKeys.yourApiKeys")}</CardTitle>
            <CardDescription>
              {t("apiKeys.keysCreated", { count: keys.length })}
            </CardDescription>
          </CardHeader>
          <CardContent>
            {keys.length === 0 ? (
              <div className="py-8 text-center text-muted-foreground">
                {t("apiKeys.noKeys")}
              </div>
            ) : (
              <TableVirtuoso
                style={{ height: "calc(100dvh - 280px)", minHeight: 400 }}
                data={keys}
                components={{
                  Table: (props) => (
                    <table
                      {...props}
                      className="w-full caption-bottom text-sm"
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
                    <th className="h-10 px-4 align-middle font-medium text-muted-foreground w-[50px]">
                      <Checkbox
                        checked={selectedKeys.length === keys.length && keys.length > 0}
                        onCheckedChange={toggleSelectAll}
                      />
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("common.name")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("apiKeys.keyPrefix")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("apiKeys.quota")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("apiKeys.restrictions")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("apiKeys.expires")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground">
                      {t("common.status")}
                    </th>
                    <th className="h-10 px-4 text-left align-middle font-medium text-muted-foreground w-[100px]">
                      {t("common.actions")}
                    </th>
                  </tr>
                )}
                itemContent={(_index, key) => (
                  <>
                    <td className="p-4 align-middle">
                      <Checkbox
                        checked={selectedKeys.includes(key.id)}
                        onCheckedChange={() => toggleSelectKey(key.id)}
                      />
                    </td>
                    <td className="p-4 align-middle font-medium">
                      <div>
                        {key.name}
                        {key.group !== "default" && (
                          <Badge variant="outline" className="ml-2 text-xs">
                            {key.group}
                          </Badge>
                        )}
                      </div>
                      {key.allowed_groups && key.allowed_groups.length > 0 && (
                        <div className="mt-1">
                          <GroupsBadge groups={key.allowed_groups} variant="secondary" />
                        </div>
                      )}
                    </td>
                    <td className="p-4 align-middle">
                      <div className="flex items-center gap-1">
                        <code className="rounded bg-muted px-2 py-0.5 text-sm">
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
                    </td>
                    <td className="p-4 align-middle">
                      {key.quota_unlimited ? (
                        <Badge variant="secondary">{t("apiKeys.unlimited")}</Badge>
                      ) : (
                        <span>{key.quota_remaining?.toLocaleString() ?? 0}</span>
                      )}
                    </td>
                    <td className="p-4 align-middle">
                      <TooltipProvider>
                        <div className="flex gap-1">
                          {key.model_limits_enabled && key.model_limits.length > 0 && (
                            <Tooltip>
                              <TooltipTrigger>
                                <Layers className="h-4 w-4 text-muted-foreground" />
                              </TooltipTrigger>
                              <TooltipContent>
                                <p>{t("apiKeys.modelLimits")}: {key.model_limits.join(", ")}</p>
                              </TooltipContent>
                            </Tooltip>
                          )}
                          {key.ip_whitelist.length > 0 && (
                            <Tooltip>
                              <TooltipTrigger>
                                <Globe className="h-4 w-4 text-muted-foreground" />
                              </TooltipTrigger>
                              <TooltipContent>
                                <p>{t("apiKeys.ipWhitelist")}: {key.ip_whitelist.join(", ")}</p>
                              </TooltipContent>
                            </Tooltip>
                          )}
                          {key.max_multiplier != null && (
                            <Tooltip>
                              <TooltipTrigger>
                                <Badge variant="outline" className="text-xs px-1.5">
                                  ≤{key.max_multiplier}x
                                </Badge>
                              </TooltipTrigger>
                              <TooltipContent>
                                <p>{t("apiKeys.maxMultiplier")}: {key.max_multiplier}x</p>
                              </TooltipContent>
                            </Tooltip>
                          )}
                          {!key.model_limits_enabled && key.ip_whitelist.length === 0 && key.max_multiplier == null && (
                            <span className="text-muted-foreground">-</span>
                          )}
                        </div>
                      </TooltipProvider>
                    </td>
                    <td className="p-4 align-middle">
                      {key.expires_at ? formatDate(key.expires_at) : t("common.never")}
                    </td>
                    <td className="p-4 align-middle">
                      <Switch
                        checked={key.enabled}
                        onCheckedChange={() => handleToggleEnabled(key)}
                      />
                    </td>
                    <td className="p-4 align-middle">
                      <div className="flex gap-1">
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
                    </td>
                  </>
                )}
              />
            )}
          </CardContent>
        </Card>
      </motion.div>

      {/* Edit Dialog */}
      <Dialog open={!!editKey} onOpenChange={(open) => { if (!open) { setEditKey(null); resetCreateForm(); } }}>
        <DialogContent className="max-w-4xl max-h-[85vh] overflow-y-auto">
          <DialogHeader>
            <DialogTitle>{t("apiKeys.editApiKey")}</DialogTitle>
            <DialogDescription>
              {t("apiKeys.editDescription")}
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4 py-4 max-h-[60vh] overflow-y-auto">
            <div className="space-y-2">
              <Label htmlFor="editName">{t("common.name")}</Label>
              <Input
                id="editName"
                value={newKeyName}
                onChange={(e) => setNewKeyName(e.target.value)}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="editGroup">{t("apiKeys.group")}</Label>
              <Input
                id="editGroup"
                value={newKeyGroup}
                onChange={(e) => setNewKeyGroup(e.target.value)}
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
                id="editQuotaUnlimited"
                checked={newKeyQuotaUnlimited}
                onCheckedChange={setNewKeyQuotaUnlimited}
              />
              <Label htmlFor="editQuotaUnlimited">{t("apiKeys.unlimitedQuota")}</Label>
            </div>
            {!newKeyQuotaUnlimited && (
              <div className="space-y-2">
                <Label htmlFor="editQuota">{t("apiKeys.quotaRemaining")}</Label>
                <Input
                  id="editQuota"
                  type="number"
                  min="0"
                  value={newKeyQuota}
                  onChange={(e) => setNewKeyQuota(e.target.value)}
                />
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
          </div>
          <DialogFooter>
            <Button variant="outline" onClick={() => { setEditKey(null); resetCreateForm(); }}>
              {t("common.cancel")}
            </Button>
            <Button onClick={handleUpdate} disabled={updating}>
              {updating ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </PageWrapper>
  );
}
