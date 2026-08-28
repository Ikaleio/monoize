import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
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
import { PAYMENT_TYPE_IDS } from "@/lib/recharge";
import {
  createPaymentChannelOptimistic,
  updatePaymentChannelOptimistic,
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
  const [form, setForm] = useState<FormState>(emptyForm());
  const [saving, setSaving] = useState(false);
  const isEdit = channel !== null;

  useEffect(() => {
    if (open) {
      setForm(channel ? formFromChannel(channel) : emptyForm());
    }
  }, [open, channel]);

  const configFields = CONFIG_FIELDS[form.type_id] ?? [];

  const setType = (type_id: string) => {
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
  };

  const errorToast = (error: Error) => {
    const message =
      error instanceof DashboardApiError
        ? t(`payments.errors.${error.code}`, { defaultValue: error.message })
        : error.message;
    toast.error(message);
  };

  const handleSubmit = async () => {
    if (saving) return;
    const name = form.name.trim();
    if (!name) {
      toast.error(t("payments.nameRequired"));
      return;
    }
    if (!isEdit) {
      // RC-P6: on create every secret field must be non-empty.
      const missingSecret = configFields.some(
        (field) => field.secret && !(form.config[field.key] ?? "").trim(),
      );
      const missingRequired = configFields.some(
        (field) =>
          !field.secret &&
          !field.optional &&
          !(form.config[field.key] ?? "").trim(),
      );
      if (missingSecret || missingRequired) {
        toast.error(t("payments.errors.invalid_channel_config"));
        return;
      }
    }
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
          errorToast,
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
          errorToast,
        );
      }
      onOpenChange(false);
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-h-[calc(100dvh-3rem)] sm:max-w-lg">
        <div className="flex min-h-0 flex-col p-6">
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
              <Label htmlFor="channel-name">{t("payments.name")}</Label>
              <Input
                id="channel-name"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor="channel-type">{t("payments.type")}</Label>
                <Select
                  value={form.type_id}
                  onValueChange={setType}
                  disabled={isEdit}
                >
                  <SelectTrigger id="channel-type">
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
                <Label htmlFor="channel-currency">
                  {t("payments.currency")}
                </Label>
                <Input
                  id="channel-currency"
                  value={form.currency}
                  disabled={isEdit || form.type_id === "epay"}
                  onChange={(e) =>
                    setForm({ ...form, currency: e.target.value })
                  }
                  placeholder="USD"
                  className="uppercase"
                />
              </div>
            </div>

            <div className="grid gap-2">
              <Label htmlFor="channel-rate">{t("payments.usdRate")}</Label>
              <Input
                id="channel-rate"
                inputMode="decimal"
                value={form.usd_rate}
                onChange={(e) => setForm({ ...form, usd_rate: e.target.value })}
                placeholder="7.30"
              />
              <p className="text-xs text-muted-foreground">
                {t("payments.usdRateHelp")}
              </p>
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor="channel-min">{t("payments.minCredit")}</Label>
                <Input
                  id="channel-min"
                  inputMode="decimal"
                  value={form.min_credit_usd}
                  onChange={(e) =>
                    setForm({ ...form, min_credit_usd: e.target.value })
                  }
                />
              </div>
              <div className="grid gap-2">
                <Label htmlFor="channel-max">{t("payments.maxCredit")}</Label>
                <Input
                  id="channel-max"
                  inputMode="decimal"
                  value={form.max_credit_usd}
                  onChange={(e) =>
                    setForm({ ...form, max_credit_usd: e.target.value })
                  }
                />
              </div>
            </div>

            {configFields.map((field) => (
              <div key={field.key} className="grid gap-2">
                <Label htmlFor={`channel-config-${field.key}`}>
                  {t(`payments.config.${form.type_id}.${field.key}`, {
                    defaultValue: field.key,
                  })}
                </Label>
                <Input
                  id={`channel-config-${field.key}`}
                  type={field.secret ? "password" : "text"}
                  autoComplete="off"
                  value={form.config[field.key] ?? ""}
                  onChange={(e) =>
                    setForm({
                      ...form,
                      config: { ...form.config, [field.key]: e.target.value },
                    })
                  }
                  placeholder={
                    field.secret && isEdit
                      ? t("payments.secretStored")
                      : undefined
                  }
                />
              </div>
            ))}

            <div className="grid gap-4 sm:grid-cols-2">
              <div className="grid gap-2">
                <Label htmlFor="channel-sort">{t("payments.sortOrder")}</Label>
                <Input
                  id="channel-sort"
                  inputMode="numeric"
                  value={form.sort_order}
                  onChange={(e) =>
                    setForm({ ...form, sort_order: e.target.value })
                  }
                />
              </div>
              <div className="flex items-center justify-between rounded-lg border p-3">
                <Label htmlFor="channel-enabled">{t("payments.enabled")}</Label>
                <Switch
                  id="channel-enabled"
                  checked={form.enabled}
                  onCheckedChange={(checked) =>
                    setForm({ ...form, enabled: checked })
                  }
                />
              </div>
            </div>
          </div>

          <DialogFooter className="shrink-0 pt-4">
            <Button type="button" onClick={handleSubmit} disabled={saving}>
              {saving ? t("common.saving") : t("common.save")}
            </Button>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );
}
