import type { SystemSettings } from "@/lib/api";
import { SettingsGroup } from "./settings-layout";
import { SwitchRow } from "./settings-rows";

interface BillingSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Settlement policy when usage or prices are missing. */
export function BillingSection({ settings, onChange }: BillingSectionProps) {
  return (
    <SettingsGroup id="billing">
      <SwitchRow
        id="monoize_enable_estimated_billing"
        checked={settings.monoize_enable_estimated_billing}
        onCheckedChange={(monoize_enable_estimated_billing) =>
          onChange({ monoize_enable_estimated_billing })
        }
      />
      <SwitchRow
        id="allow_free_when_unpriced"
        checked={settings.allow_free_when_unpriced}
        onCheckedChange={(allow_free_when_unpriced) => onChange({ allow_free_when_unpriced })}
      />
      <SwitchRow
        id="allow_free_when_missing_usage"
        checked={settings.allow_free_when_missing_usage}
        onCheckedChange={(allow_free_when_missing_usage) =>
          onChange({ allow_free_when_missing_usage })
        }
      />
    </SettingsGroup>
  );
}
