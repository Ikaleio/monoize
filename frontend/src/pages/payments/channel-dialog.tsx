import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { DashboardApiError } from "@/lib/api";
import type { PaymentChannel } from "@/lib/api";
import { toastApiError } from "@/lib/api-error";
import { PAYMENT_TYPE_IDS, parseUsdToNano } from "@/lib/recharge";
import {
  createPaymentChannelOptimistic,
  updatePaymentChannelOptimistic,
  useSettings,
} from "@/lib/swr";

interface ConfigField {
  key: string;
  secret: boolean;
  optional?: boolean;
}

/** RC-P5 config schemas, keyed by adapter `type_id`. */
const CONFIG_FIELDS: Record<string, ConfigField[]> = {
  epay: [
    { key: "gateway_url", secret: false },
    { key: "merchant_id", secret: false },
    { key: "merchant_key", secret: true },
    { key: "pay_type", secret: false, optional: true },
  ],
  stripe: [
    { key: "secret_key", secret: true },
    { key: "webhook_secret", secret: true },
  ],
};

/** RC-U5 decimal: positive, ≤12 integer digits, ≤9 fractional digits. */
const RATE_PATTERN = /^(0|[1-9]\d{0,11})(\.\d{1,9})?$/;

interface FormState {
  name: string;
  type_id: string;
  currency: string;
  usd_rate: string;
  min_credit_usd: string;
  max_credit_usd: string;
  sort_order: string;
  enabled: boolean;
  config: Record<string, string>;
}

/** Inline errors keyed by form field; config fields use `config.<key>`. */
type FieldErrors = Record<string, string>;

function emptyForm(): FormState {
  return {
    name: "",
    type_id: "epay",
    currency: "CNY",
    usd_rate: "",
    min_credit_usd: "1",
    max_credit_usd: "10000",
    sort_order: "0",
    enabled: true,
    config: {},
  };
}

function formFromChannel(channel: PaymentChannel): FormState {
  return {
    name: channel.name,
    type_id: channel.type_id,
    currency: channel.currency,
    usd_rate: channel.usd_rate,
    min_credit_usd: channel.min_credit_usd,
    max_credit_usd: channel.max_credit_usd,
    sort_order: String(channel.sort_order),
    enabled: channel.enabled,
    config: { ...channel.config },
  };
}

interface ChannelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** null → create; non-null → edit (RC-D1: type/currency immutable). */
  channel: PaymentChannel | null;
  channels: PaymentChannel[];
}

/**
 * RC-M2 create/edit dialog. Config fields derive from the RC-P5 schema of the
 * selected `type_id`; secret inputs render empty with a "stored" placeholder
 * on edit, and an empty secret submit keeps the stored value (RC-P6).
 */
export function ChannelDialog({
  open,
  onOpenChange,
  channel,
  channels,
}: ChannelDialogProps) {
  const { t } = useTranslation();
  const idPrefix = useId();
  const fieldId = (key: string) => `${idPrefix}-${key.replace(".", "-")}`;
  const { data: settings } = useSettings();
  const [initialForm, setInitialForm] = useState<FormState>(emptyForm());
  const [form, setForm] = useState<FormState>(emptyForm());
  const [errors, setErrors] = useState<FieldErrors>({});
  const [saving, setSaving] = useState(false);
  const isEdit = channel !== null;
  const dirty = JSON.stringify(form) !== JSON.stringify(initialForm);

  useEffect(() => {
    if (open) {
      const next = channel ? formFromChannel(channel) : emptyForm();
      setInitialForm(next);
      setForm(next);
      setErrors({});
    }
  }, [open, channel]);

  const configFields = CONFIG_FIELDS[form.type_id] ?? [];
  const origin = settings?.recharge_public_origin.trim() ?? "";
  const notifyUrl =
    isEdit && channel.type_id === "stripe" && origin
      ? `${origin}/api/pay/notify/${channel.id}`
      : null;

  const update = (patch: Partial<FormState>, clear: string[]) => {
    setForm((prev) => ({ ...prev, ...patch }));
    setErrors((prev) => {
      const next = { ...prev };
      for (const key of clear) delete next[key];
      return next;
    });
  };

  const setConfig = (key: string, value: string) => {
    setForm((prev) => ({ ...prev, config: { ...prev.config, [key]: value } }));
    setErrors((prev) => {
      const next = { ...prev };
      delete next[`config.${key}`];
      return next;
    });
  };

  const setType = (type_id: string) => {
    // Radix Select inside a <form> echoes "" through onValueChange when its controlled
    // value changes before the items mount; that would wipe the type, currency and config.
    if (!type_id) return;
    setForm((prev) => ({
      ...prev,
      type_id,
      currency:
        type_id === "epay"
          ? "CNY"
          : prev.currency === "CNY"
            ? "USD"
            : prev.currency,
      config: {},
    }));
    setErrors((prev) =>
      Object.fromEntries(
        Object.entries(prev).filter(([key]) => !key.startsWith("config.")),
      ),
    );
  };

  const validate = (): FieldErrors => {
    const next: FieldErrors = {};
    if (!form.name.trim()) next.name = t("payments.nameRequired");
    const rate = form.usd_rate.trim();
    if (!RATE_PATTERN.test(rate) || !/[1-9]/.test(rate)) {
      next.usd_rate = t("payments.fieldErrors.rate");
    }
    const min = parseUsdToNano(form.min_credit_usd);
    const max = parseUsdToNano(form.max_credit_usd);
    if (min === null) next.min_credit_usd = t("payments.fieldErrors.amount");
    if (max === null) next.max_credit_usd = t("payments.fieldErrors.amount");
    else if (min !== null && min > max) {
      next.max_credit_usd = t("payments.fieldErrors.maxBelowMin");
    }
    for (const field of configFields) {
      if (field.optional) continue;
      // RC-P6: an empty secret on edit keeps the stored value.
      if (field.secret && isEdit) continue;
      if (!(form.config[field.key] ?? "").trim()) {
        next[`config.${field.key}`] = t("payments.fieldErrors.required");
      }
    }
    return next;
  };

  const handleSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (saving) return;
    const nextErrors = validate();
    setErrors(nextErrors);
    const firstInvalid = [
      "name",
      "usd_rate",
      "min_credit_usd",
      "max_credit_usd",
      ...configFields.map((field) => `config.${field.key}`),
    ].find((key) => nextErrors[key]);
    if (firstInvalid) {
      document.getElementById(fieldId(firstInvalid))?.focus();
      return;
    }
    const name = form.name.trim();
    const config: Record<string, string> = {};
    for (const field of configFields) {
      config[field.key] = form.config[field.key] ?? "";
    }
    setSaving(true);
    try {
      if (isEdit) {
        await updatePaymentChannelOptimistic(
          channel.id,
          {
            name,
            usd_rate: form.usd_rate.trim(),
            min_credit_usd: form.min_credit_usd.trim(),
            max_credit_usd: form.max_credit_usd.trim(),
            enabled: form.enabled,
            sort_order: Number(form.sort_order) || 0,
            config,
          },
          channels,
        );
      } else {
        await createPaymentChannelOptimistic(
          {
            name,
            type_id: form.type_id,
            currency: form.currency.trim().toUpperCase(),
            usd_rate: form.usd_rate.trim(),
            min_credit_usd: form.min_credit_usd.trim(),
            max_credit_usd: form.max_credit_usd.trim(),
            enabled: form.enabled,
            sort_order: Number(form.sort_order) || 0,
            config,
          },
          channels,
        );
      }
      onOpenChange(false);
      toast.success(t("payments.saveSuccess"));
    } catch (error) {
      // `channel_name_exists` / `invalid_channel_config` are shared with Provider
      // Channels, whose apiErrors text names a Channel; use payment-channel wording.
      const code = error instanceof DashboardApiError ? error.code : null;
      if (code === "channel_name_exists") {
        setErrors({ name: t("payments.nameExists") });
        document.getElementById(fieldId("name"))?.focus();
      } else if (code === "invalid_channel_config") {
        toast.error(t("payments.saveFailed"), {
          description: t("payments.invalidConfig"),
        });
      } else {
        toastApiError(error, t, t("payments.saveFailed"));
      }
    } finally {
      setSaving(false);
    }
  };

  const errorId = (key: string) => `${fieldId(key)}-error`;
  const invalidProps = (key: string) => ({
    "aria-invalid": errors[key] ? true : undefined,
    "aria-describedby": errors[key] ? errorId(key) : undefined,
  });
  const fieldErrorNode = (key: string) =>
    errors[key] ? <FieldError id={errorId(key)}>{errors[key]}</FieldError> : null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        dirty={dirty && !saving}
        className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-lg"
      >
        <form
          noValidate
          onSubmit={handleSubmit}
          className="flex min-h-0 flex-col p-6"
        >
          <DialogHeader className="shrink-0">
            <DialogTitle>
              {isEdit ? t("payments.edit") : t("payments.create")}
            </DialogTitle>
            <DialogDescription>
              {isEdit ? channel.name : t("payments.createDescription")}
            </DialogDescription>
          </DialogHeader>

          <div className="grid min-h-0 flex-1 gap-4 overflow-y-auto py-2 pr-1">
            <div className="grid gap-2">
              <Label htmlFor={fieldId("name")}>{t("payments.name")}</Label>
              <Input
                id={fieldId("name")}
                value={form.name}
                onChange={(e) => update({ name: e.target.value }, ["name"])}
                {...invalidProps("name")}
              />
              {fieldErrorNode("name")}
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor={fieldId("type")}>{t("payments.type")}</Label>
                <Select
                  value={form.type_id}
                  onValueChange={setType}
                  disabled={isEdit}
                >
                  <SelectTrigger id={fieldId("type")}>
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {PAYMENT_TYPE_IDS.map((typeId) => (
                      <SelectItem key={typeId} value={typeId}>
                        {typeId}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className="grid gap-2">
                <Label htmlFor={fieldId("currency")}>
                  {t("payments.currency")}
                </Label>
                <Input
                  id={fieldId("currency")}
                  value={form.currency}
                  disabled={isEdit || form.type_id === "epay"}
                  onChange={(e) => update({ currency: e.target.value }, [])}
                  placeholder="USD"
                  className="uppercase"
                />
              </div>
            </div>

            <div className="grid gap-2">
              <Label htmlFor={fieldId("usd_rate")}>{t("payments.usdRate")}</Label>
              <Input
                id={fieldId("usd_rate")}
                inputMode="decimal"
                value={form.usd_rate}
                onChange={(e) => update({ usd_rate: e.target.value }, ["usd_rate"])}
                placeholder={t("payments.ratePlaceholder")}
                {...invalidProps("usd_rate")}
              />
              <p className="text-xs text-muted-foreground">{t("payments.usdRateHelp")}</p>
              {fieldErrorNode("usd_rate")}
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor={fieldId("min_credit_usd")}>
                  {t("payments.minCredit")}
                </Label>
                <Input
                  id={fieldId("min_credit_usd")}
                  inputMode="decimal"
                  value={form.min_credit_usd}
                  onChange={(e) =>
                    update({ min_credit_usd: e.target.value }, [
                      "min_credit_usd",
                      "max_credit_usd",
                    ])
                  }
                  {...invalidProps("min_credit_usd")}
                />
                {fieldErrorNode("min_credit_usd")}
              </div>
              <div className="grid gap-2">
                <Label htmlFor={fieldId("max_credit_usd")}>
                  {t("payments.maxCredit")}
                </Label>
                <Input
                  id={fieldId("max_credit_usd")}
                  inputMode="decimal"
                  value={form.max_credit_usd}
                  onChange={(e) =>
                    update({ max_credit_usd: e.target.value }, ["max_credit_usd"])
                  }
                  {...invalidProps("max_credit_usd")}
                />
                {fieldErrorNode("max_credit_usd")}
              </div>
            </div>

            {configFields.map((field) => {
              const key = `config.${field.key}`;
              return (
                <div key={field.key} className="grid gap-2">
                  <Label htmlFor={fieldId(key)}>
                    {t(`payments.config.${form.type_id}.${field.key}`, {
                      defaultValue: field.key,
                    })}
                  </Label>
                  <Input
                    id={fieldId(key)}
                    type={field.secret ? "password" : "text"}
                    autoComplete="off"
                    value={form.config[field.key] ?? ""}
                    onChange={(e) => setConfig(field.key, e.target.value)}
                    placeholder={
                      field.secret && isEdit
                        ? t("payments.secretStored")
                        : undefined
                    }
                    {...invalidProps(key)}
                  />
                  {fieldErrorNode(key)}
                </div>
              );
            })}

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor={fieldId("sort_order")}>
                  {t("payments.sortOrder")}
                </Label>
                <Input
                  id={fieldId("sort_order")}
                  inputMode="numeric"
                  value={form.sort_order}
                  onChange={(e) => update({ sort_order: e.target.value }, [])}
                />
              </div>
              <div className="flex items-center justify-between rounded-lg border p-3">
                <Label htmlFor={fieldId("enabled")}>{t("payments.enabled")}</Label>
                <Switch
                  id={fieldId("enabled")}
                  checked={form.enabled}
                  onCheckedChange={(checked) => update({ enabled: checked }, [])}
                />
              </div>
            </div>

            {isEdit && channel.type_id === "stripe" ? (
              <div className="grid gap-2">
                <Label htmlFor={fieldId("notify_url")}>
                  {t("payments.notifyUrl")}
                </Label>
                {notifyUrl ? (
                  <div className="flex min-w-0 items-center gap-1">
                    <Input
                      id={fieldId("notify_url")}
                      readOnly
                      value={notifyUrl}
                      className="font-mono"
                    />
                    <CopyButton value={notifyUrl} label={t("payments.copyNotifyUrl")} />
                  </div>
                ) : null}
                <p className="text-xs text-muted-foreground">
                  {notifyUrl
                    ? t("payments.notifyUrlHelp")
                    : t("payments.notifyUrlOriginUnset")}
                </p>
              </div>
            ) : null}
          </div>

          <DialogFooter className="shrink-0 gap-2 pt-4">
            <Button
              type="button"
              variant="outline"
              disabled={saving}
              onClick={() => onOpenChange(false)}
            >
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={saving}>
              {saving ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
