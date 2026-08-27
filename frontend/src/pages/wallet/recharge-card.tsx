import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ArrowRight, CreditCard } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { springs } from "@/components/ui/motion";
import { DashboardApiError } from "@/lib/api";
import { parseUsdToNano, previewPayAmount } from "@/lib/recharge";
import { createRechargeOrderOptimistic, useRechargeChannels } from "@/lib/swr";

const PRESET_AMOUNTS = ["5", "10", "25", "50", "100"];

interface RechargeCardProps {
  /** SWR key of the first orders page, for the RC-W3 optimistic insert. */
  ordersFirstPageKey: string;
}

/**
 * RC-W3: channel selector, USD presets, custom amount, exact pay preview,
 * and submit that navigates top-level to the provider payment URL.
 */
export function RechargeCard({ ordersFirstPageKey }: RechargeCardProps) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const { data: channels, isLoading } = useRechargeChannels();
  const [channelId, setChannelId] = useState<string | null>(null);
  const [amount, setAmount] = useState("10");
  const [submitting, setSubmitting] = useState(false);

  const channel = useMemo(() => {
    if (!channels || channels.length === 0) return null;
    return channels.find((c) => c.id === channelId) ?? channels[0];
  }, [channels, channelId]);

  const bounds = useMemo(() => {
    if (!channel) return null;
    const min = parseUsdToNano(channel.min_credit_usd);
    const max = parseUsdToNano(channel.max_credit_usd);
    return min !== null && max !== null ? { min, max } : null;
  }, [channel]);

  const amountNano = useMemo(() => parseUsdToNano(amount), [amount]);
  const inRange =
    amountNano !== null &&
    bounds !== null &&
    amountNano >= bounds.min &&
    amountNano <= bounds.max;
  const preview =
    channel && inRange
      ? previewPayAmount(amount, channel.usd_rate, channel.pay_scale)
      : null;

  const presetDisabled = (preset: string) => {
    if (!bounds) return true;
    const nano = parseUsdToNano(preset);
    return nano === null || nano < bounds.min || nano > bounds.max;
  };

  const handleSubmit = async () => {
    if (!channel || amountNano === null || !inRange || submitting) return;
    setSubmitting(true);
    try {
      const result = await createRechargeOrderOptimistic(
        {
          payment_channel_id: channel.id,
          credit_nano_usd: amountNano.toString(),
        },
        ordersFirstPageKey,
      );
      // RC-W3: top-level navigation to the provider, never an iframe.
      window.location.assign(result.payment.url);
    } catch (error) {
      // Entered channel/amount state is intentionally kept intact (RC-W3).
      setSubmitting(false);
      const message =
        error instanceof DashboardApiError
          ? t(`wallet.errors.${error.code}`, { defaultValue: error.message })
          : error instanceof Error
            ? error.message
            : t("common.error");
      toast.error(message);
    }
  };

  return (
    <Card className="flex flex-col">
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          <CreditCard className="h-4 w-4" />
          {t("wallet.rechargeTitle")}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-1 flex-col gap-4">
        {isLoading ? (
          <div className="flex flex-col gap-3" aria-busy="true">
            <Skeleton className="h-9 w-full" />
            <Skeleton className="h-9 w-3/4" />
            <Skeleton className="h-9 w-full" />
          </div>
        ) : !channels || channels.length === 0 ? (
          <EmptyState
            variant="inline"
            title={t("wallet.noChannelsTitle")}
            description={t("wallet.noChannelsDescription")}
          />
        ) : (
          <motion.div
            initial={reduced ? { opacity: 0 } : { opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            transition={reduced ? { duration: 0.15 } : springs.smooth}
            className="flex flex-1 flex-col gap-4"
          >
            <div className="grid gap-2">
              <Label htmlFor="recharge-channel">{t("wallet.channel")}</Label>
              <Select
                value={channel?.id ?? ""}
                onValueChange={(value) => setChannelId(value)}
              >
                <SelectTrigger id="recharge-channel">
                  <SelectValue placeholder={t("wallet.channelPlaceholder")} />
                </SelectTrigger>
                <SelectContent>
                  {channels.map((c) => (
                    <SelectItem key={c.id} value={c.id}>
                      {c.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="grid gap-2">
              <Label htmlFor="recharge-amount">{t("wallet.amountUsd")}</Label>
              <div className="flex flex-wrap gap-1.5">
                {PRESET_AMOUNTS.map((preset) => (
                  <motion.span
                    key={preset}
                    whileTap={reduced ? undefined : { scale: 0.94 }}
                    transition={springs.snappy}
                    className="inline-flex"
                  >
                    <Button
                      type="button"
                      variant={amount === preset ? "secondary" : "outline"}
                      size="sm"
                      className="h-8 px-3 tabular-nums"
                      disabled={presetDisabled(preset)}
                      onClick={() => setAmount(preset)}
                    >
                      ${preset}
                    </Button>
                  </motion.span>
                ))}
              </div>
              <Input
                id="recharge-amount"
                inputMode="decimal"
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
                placeholder={t("wallet.customAmount")}
                aria-invalid={amount !== "" && !inRange}
              />
              {channel && (
                <p className="text-xs text-muted-foreground">
                  {t("wallet.amountRange", {
                    min: channel.min_credit_usd,
                    max: channel.max_credit_usd,
                  })}
                </p>
              )}
            </div>

            <div className="mt-auto flex items-center justify-between gap-3 rounded-lg border bg-muted/40 px-3 py-2.5">
              <span className="text-sm text-muted-foreground">
                {t("wallet.youPay")}
              </span>
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span
                  key={preview ? `${preview}-${channel?.currency}` : "invalid"}
                  initial={reduced ? { opacity: 0 } : { opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={reduced ? { opacity: 0 } : { opacity: 0, y: -10 }}
                  transition={reduced ? { duration: 0.15 } : springs.snappy}
                  className="text-sm font-semibold tabular-nums"
                >
                  {preview && channel ? `${preview} ${channel.currency}` : "—"}
                </motion.span>
              </AnimatePresence>
            </div>

            <Button
              type="button"
              className="w-full"
              disabled={!inRange || submitting}
              onClick={handleSubmit}
            >
              {submitting ? t("wallet.redirecting") : t("wallet.rechargeSubmit")}
              {!submitting && <ArrowRight className="ml-2 h-4 w-4" />}
            </Button>
          </motion.div>
        )}
      </CardContent>
    </Card>
  );
}
