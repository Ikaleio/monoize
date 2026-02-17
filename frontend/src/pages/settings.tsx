import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, Save, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Switch } from "@/components/ui/switch";
import { Separator } from "@/components/ui/separator";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useSettings, updateSettingsOptimistic } from "@/lib/swr";
import type { SystemSettings } from "@/lib/api";
import { PageWrapper, motion, transitions, StaggerList, StaggerItem } from "@/components/ui/motion";

const EFFORT_VALUES = ["none", "minimum", "low", "medium", "high", "xhigh", "max"] as const;

interface SuffixRow {
  id: number;
  suffix: string;
  effort: string;
}

let suffixRowId = 0;

function mapToRows(map: Record<string, string> | undefined): SuffixRow[] {
  return Object.entries(map ?? {}).map(([suffix, effort]) => ({
    id: ++suffixRowId,
    suffix,
    effort,
  }));
}

function rowsToMap(rows: SuffixRow[]): Record<string, string> {
  const map: Record<string, string> = {};
  for (const row of rows) {
    if (row.suffix) map[row.suffix] = row.effort;
  }
  return map;
}

function SuffixMapEditor({
  value,
  onChange,
}: {
  value: Record<string, string> | undefined;
  onChange: (map: Record<string, string>) => void;
}) {
  const { t } = useTranslation();
  const [rows, setRows] = useState<SuffixRow[]>(() => mapToRows(value));
  const prevValueRef = useRef(value);

  useEffect(() => {
    if (prevValueRef.current !== value) {
      prevValueRef.current = value;
      setRows(mapToRows(value));
    }
  }, [value]);

  const commit = useCallback(
    (updated: SuffixRow[]) => {
      setRows(updated);
      onChange(rowsToMap(updated));
    },
    [onChange]
  );

  return (
    <div className="space-y-4">
      {rows.map((row, idx) => (
        <div key={row.id} className="flex items-center gap-2">
          <Input
            defaultValue={row.suffix}
            placeholder={t("settings.suffix")}
            className="flex-1 transition-all focus:scale-[1.01]"
            onBlur={(e) => {
              const updated = rows.map((r, i) =>
                i === idx ? { ...r, suffix: e.target.value } : r
              );
              commit(updated);
            }}
          />
          <Select
            value={row.effort}
            onValueChange={(val) => {
              const updated = rows.map((r, i) =>
                i === idx ? { ...r, effort: val } : r
              );
              commit(updated);
            }}
          >
            <SelectTrigger className="w-[140px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {EFFORT_VALUES.map((v) => (
                <SelectItem key={v} value={v}>
                  {v}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => commit(rows.filter((_, i) => i !== idx))}
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
      ))}
      <Button
        variant="outline"
        size="sm"
        onClick={() => {
          setRows([...rows, { id: ++suffixRowId, suffix: "", effort: "high" }]);
        }}
      >
        <Plus className="mr-2 h-4 w-4" />
        {t("settings.addSuffix")}
      </Button>
      <p className="text-sm text-muted-foreground">
        {t("settings.effortValues")}
      </p>
    </div>
  );
}

export function SettingsPage() {
  const { t } = useTranslation();
  const { data: settings, isLoading, mutate } = useSettings();
  const [localSettings, setLocalSettings] = useState<SystemSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  // Use local state if user has made changes, otherwise use SWR data
  const currentSettings = localSettings ?? settings;

  const handleChange = (updates: Partial<SystemSettings>) => {
    if (!currentSettings) return;
    setLocalSettings({ ...currentSettings, ...updates });
  };

  const handleSave = async () => {
    if (!currentSettings) return;
    setSaving(true);
    try {
      await updateSettingsOptimistic(currentSettings, (error) => {
        console.error(t("settings.failedSave"), error);
      });
      setLocalSettings(null); // Clear local state to use SWR data
      setSaved(true);
      setTimeout(() => setSaved(false), 2000);
      mutate();
    } catch {
      // Error already handled by optimistic update
    } finally {
      setSaving(false);
    }
  };

  const hasChanges = localSettings !== null;

  if (isLoading) {
    return (
      <div className="space-y-6">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-64" />
      </div>
    );
  }

  if (!currentSettings) {
    return (
      <div className="py-8 text-center text-muted-foreground">
        {t("settings.failedLoad")}
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
          <h1 className="text-3xl font-bold tracking-tight">{t("settings.title")}</h1>
          <p className="text-muted-foreground">{t("settings.description")}</p>
        </div>
        <motion.div
          whileHover={{ scale: 1.02 }}
          whileTap={{ scale: 0.98 }}
        >
          <Button onClick={handleSave} disabled={saving || !hasChanges}>
            <Save className="mr-2 h-4 w-4" />
            {saving ? t("common.saving") : saved ? t("common.saved") : t("common.saveChanges")}
          </Button>
        </motion.div>
      </motion.div>

      <StaggerList className="grid gap-6">
        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.siteInformation")}</CardTitle>
              <CardDescription>{t("settings.siteInfoDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="site_name">{t("settings.siteName")}</Label>
                <Input
                  id="site_name"
                  value={currentSettings.site_name}
                  onChange={(e) => handleChange({ site_name: e.target.value })}
                  className="transition-all focus:scale-[1.01]"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="site_description">{t("settings.siteDescription")}</Label>
                <Input
                  id="site_description"
                  value={currentSettings.site_description}
                  onChange={(e) => handleChange({ site_description: e.target.value })}
                  className="transition-all focus:scale-[1.01]"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="api_base_url">{t("settings.apiBaseUrl")}</Label>
                <Input
                  id="api_base_url"
                  value={currentSettings.api_base_url}
                  onChange={(e) => handleChange({ api_base_url: e.target.value })}
                  placeholder={t("settings.apiBaseUrlPlaceholder")}
                  className="transition-all focus:scale-[1.01]"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.apiBaseUrlDescription")}
                </p>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.registration")}</CardTitle>
              <CardDescription>{t("settings.registrationDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <motion.div
                whileHover={{ x: 4 }}
                transition={{ type: "spring", stiffness: 300 }}
                className="flex items-center justify-between"
              >
                <div className="space-y-0.5">
                  <Label>{t("settings.allowRegistration")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("settings.allowRegistrationDescription")}
                  </p>
                </div>
                <Switch
                  checked={currentSettings.registration_enabled}
                  onCheckedChange={(checked) =>
                    handleChange({ registration_enabled: checked })
                  }
                />
              </motion.div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="default_role">{t("settings.defaultUserRole")}</Label>
                <Input
                  id="default_role"
                  value={currentSettings.default_user_role}
                  onChange={(e) =>
                    handleChange({ default_user_role: e.target.value })
                  }
                  className="transition-all focus:scale-[1.01]"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.defaultUserRoleDescription")}
                </p>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.sessionSecurity")}</CardTitle>
              <CardDescription>{t("settings.sessionSecurityDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="session_ttl">{t("settings.sessionDuration")}</Label>
                <Input
                  id="session_ttl"
                  type="number"
                  min="1"
                  value={currentSettings.session_ttl_days}
                  onChange={(e) =>
                    handleChange({
                      session_ttl_days: parseInt(e.target.value) || 7,
                    })
                  }
                  className="transition-all focus:scale-[1.01]"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.sessionDurationDescription")}
                </p>
              </div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="max_api_keys">{t("settings.maxApiKeys")}</Label>
                <Input
                  id="max_api_keys"
                  type="number"
                  min="1"
                  value={currentSettings.api_key_max_per_user}
                  onChange={(e) =>
                    handleChange({
                      api_key_max_per_user: parseInt(e.target.value) || 10,
                    })
                  }
                  className="transition-all focus:scale-[1.01]"
                />
                <p className="text-sm text-muted-foreground">
                  {t("settings.maxApiKeysDescription")}
                </p>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle>{t("settings.reasoningSuffixMap")}</CardTitle>
              <CardDescription>{t("settings.reasoningSuffixMapDescription")}</CardDescription>
            </CardHeader>
            <CardContent>
              <SuffixMapEditor
                value={currentSettings.reasoning_suffix_map}
                onChange={(map) => handleChange({ reasoning_suffix_map: map })}
              />
            </CardContent>
          </Card>
        </StaggerItem>
      </StaggerList>
    </PageWrapper>
  );
}
