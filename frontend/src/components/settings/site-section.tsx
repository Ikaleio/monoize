import { useTranslation } from "react-i18next";

import {
  Field,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { SystemSettings } from "@/lib/api";

interface SiteSectionProps {
  settings: SystemSettings;
  onChange: (updates: Partial<SystemSettings>) => void;
}

/** Site identity fields: name, description, downstream API base URL. */
export function SiteSection({ settings, onChange }: SiteSectionProps) {
  const { t } = useTranslation();

  return (
    <div className="grid gap-6 sm:grid-cols-2">
      <Field>
        <FieldLabel htmlFor="site_name">{t("settings.siteName")}</FieldLabel>
        <Input
          id="site_name"
          value={settings.site_name}
          onChange={(e) => onChange({ site_name: e.target.value })}
        />
      </Field>
      <Field>
        <FieldLabel htmlFor="site_description">{t("settings.siteDescription")}</FieldLabel>
        <Input
          id="site_description"
          value={settings.site_description}
          onChange={(e) => onChange({ site_description: e.target.value })}
        />
      </Field>
      <Field className="sm:col-span-2">
        <FieldLabel htmlFor="api_base_url">{t("settings.apiBaseUrl")}</FieldLabel>
        <Input
          id="api_base_url"
          value={settings.api_base_url}
          onChange={(e) => onChange({ api_base_url: e.target.value })}
          placeholder={t("settings.apiBaseUrlPlaceholder")}
        />
        <FieldDescription>{t("settings.apiBaseUrlDescription")}</FieldDescription>
      </Field>
      <Field className="sm:col-span-2">
        <FieldLabel htmlFor="recharge_public_origin">
          {t("settings.rechargePublicOrigin")}
        </FieldLabel>
        <Input
          id="recharge_public_origin"
          value={settings.recharge_public_origin}
          onChange={(e) => onChange({ recharge_public_origin: e.target.value })}
          placeholder={t("settings.rechargePublicOriginPlaceholder")}
        />
        <FieldDescription>
          {t("settings.rechargePublicOriginDescription")}
        </FieldDescription>
      </Field>
    </div>
  );
}
