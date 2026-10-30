import type { SystemSettings } from "@/lib/api";
import { SettingsGroup } from "./settings-layout";
import { NumberRow, SwitchRow } from "./settings-rows";

const BYTES_PER_MIB = 1048576;

interface PrivacySectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Request capture and upstream error masking. */
export function PrivacySection({ settings, onChange }: PrivacySectionProps) {
  return (
    <SettingsGroup id="privacy">
      <SwitchRow
        id="monoize_request_capture_enabled"
        checked={settings.monoize_request_capture_enabled}
        onCheckedChange={(monoize_request_capture_enabled) =>
          onChange({ monoize_request_capture_enabled })
        }
      />
      {/* SSU-2a: edited in MiB, stored in bytes; 0 disables the budget. */}
      <NumberRow
        id="monoize_request_capture_max_total_bytes"
        value={Math.round(settings.monoize_request_capture_max_total_bytes / BYTES_PER_MIB)}
        onValueChange={(mib) =>
          onChange({ monoize_request_capture_max_total_bytes: mib * BYTES_PER_MIB })
        }
      />
      <SwitchRow
        id="monoize_mask_sensitive_info"
        checked={settings.monoize_mask_sensitive_info}
        onCheckedChange={(monoize_mask_sensitive_info) =>
          onChange({ monoize_mask_sensitive_info })
        }
      />
    </SettingsGroup>
  );
}
