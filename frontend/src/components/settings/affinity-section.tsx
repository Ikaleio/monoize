import { useTranslation } from "react-i18next";

import {
  Field,
  FieldContent,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import type { SystemSettings } from "@/lib/api";

interface AffinitySectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Routing affinity drawer: master switch plus a dense 3-column tuning grid. */
export function AffinitySection({ settings, onChange }: AffinitySectionProps) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-6">
      <Field orientation="horizontal">
        <FieldContent>
          <FieldLabel htmlFor="affinity_enabled">
            {t("settings.affinityEnabled")}
          </FieldLabel>
          <FieldDescription>{t("settings.affinityEnabledDescription")}</FieldDescription>
        </FieldContent>
        <Switch
          id="affinity_enabled"
          checked={settings.monoize_affinity_enabled}
          onCheckedChange={(checked) => onChange({ monoize_affinity_enabled: checked })}
        />
      </Field>
      <Separator />
      <div className="grid gap-6 sm:grid-cols-2 xl:grid-cols-3">
        <Field>
          <FieldLabel htmlFor="affinity_failback_mode">
            {t("settings.affinityFailbackMode")}
          </FieldLabel>
          <Select
            value={settings.monoize_affinity_failback_mode}
            onValueChange={(value: SystemSettings["monoize_affinity_failback_mode"]) =>
              onChange({ monoize_affinity_failback_mode: value })
            }
          >
            <SelectTrigger id="affinity_failback_mode">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectGroup>
                <SelectItem value="sticky">{t("settings.affinitySticky")}</SelectItem>
                <SelectItem value="prefer_higher_priority">
                  {t("settings.affinityPreferHigherPriority")}
                </SelectItem>
              </SelectGroup>
            </SelectContent>
          </Select>
          <FieldDescription>
            {t("settings.affinityFailbackModeDescription")}
          </FieldDescription>
        </Field>
        <Field>
          <FieldLabel htmlFor="affinity_idle_ttl_seconds">
            {t("settings.affinityIdleTtlSeconds")}
          </FieldLabel>
          <Input
            id="affinity_idle_ttl_seconds"
            type="number"
            min="1"
            value={settings.monoize_affinity_idle_ttl_seconds}
            onChange={(event) =>
              onChange({
                monoize_affinity_idle_ttl_seconds: Math.max(
                  1,
                  parseInt(event.target.value) || 1
                ),
              })
            }
          />
          <FieldDescription>
            {t("settings.affinityIdleTtlSecondsDescription")}
          </FieldDescription>
        </Field>
        <Field>
          <FieldLabel htmlFor="affinity_failback_delay_seconds">
            {t("settings.affinityFailbackDelaySeconds")}
          </FieldLabel>
          <Input
            id="affinity_failback_delay_seconds"
            type="number"
            min="0"
            value={settings.monoize_affinity_failback_delay_seconds}
            onChange={(event) =>
              onChange({
                monoize_affinity_failback_delay_seconds: Math.max(
                  0,
                  parseInt(event.target.value) || 0
                ),
              })
            }
          />
          <FieldDescription>
            {t("settings.affinityFailbackDelaySecondsDescription")}
          </FieldDescription>
        </Field>
      </div>
    </div>
  );
}
