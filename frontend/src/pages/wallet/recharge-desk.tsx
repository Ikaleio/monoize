import { Link } from "react-router-dom";
import { useAuth } from "@/hooks/use-auth";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ArrowUpRight, Landmark, LoaderCircle } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Field,
  FieldDescription,
  FieldGroup,
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
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { springs } from "@/components/ui/motion";
import { DashboardApiError } from "@/lib/api";
import { formatNanoUsd } from "@/lib/exact-decimal";
import { parseUsdToNano, previewPayAmount } from "@/lib/recharge";
import { createRechargeOrderOptimistic, useRechargeChannels } from "@/lib/swr";
import { WalletFeedback } from "./wallet-feedback";

const PRESET_AMOUNTS = ["5", "10", "25", "50", "100"];

function RechargeDeskSkeleton() {
  return (
    <div
      className="grid gap-6 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]"
      aria-busy="true"
    >
      <div className="flex flex-col gap-5">
        <Skeleton className="h-5 w-32" />
        <div className="grid grid-cols-5 gap-2">
          {PRESET_AMOUNTS.map((amount) => (
            <Skeleton key={amount} className="h-11" />
          ))}
        </div>
        <Skeleton className="h-11 w-full bg-wallet-action text-wallet-action-foreground hover:bg-wallet-action/90" />
        <Skeleton className="h-11 w-full bg-wallet-action text-wallet-action-foreground hover:bg-wallet-action/90" />
      </div>
      <div className="flex flex-col gap-5">
        <Skeleton className="h-5 w-32" />
        <Skeleton className="h-14 w-full" />
        <Skeleton className="h-10 w-40" />
        <Skeleton className="h-11 w-full bg-wallet-action text-wallet-action-foreground hover:bg-wallet-action/90" />
      </div>
    </div>
  );
}

export function RechargeDesk({
  ordersFirstPageKey,
}: {
  ordersFirstPageKey: string;
}) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const { user } = useAuth();
  const isAdmin = user?.role === "admin" || user?.role === "super_admin";
  const { data: channels, error, isLoading, mutate } = useRechargeChannels();
  const [channelId, setChannelId] = useState<string | null>(null);
  const [amount, setAmount] = useState("10");
  const [submitting, setSubmitting] = useState(false);
  const channel = useMemo(() => {
    if (!channels?.length) return null;
    return (
      channels.find((candidate) => candidate.id === channelId) ?? channels[0]
    );
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
  const invalidAmount = amount !== "" && !inRange;
  const validationMessage =
    amountNano === null
      ? t("wallet.errors.invalid_amount")
      : t("wallet.errors.amount_out_of_range");

  const handleSubmit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
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
      window.location.assign(result.payment.url);
    } catch (caught) {
      setSubmitting(false);
      toast.error(
        caught instanceof DashboardApiError
          ? t(`wallet.errors.${caught.code}`, {
              defaultValue: t("wallet.errors.request_failed"),
            })
          : t("wallet.errors.request_failed"),
      );
    }
  };

  return (
    <Card
      role="region"
      aria-labelledby="wallet-recharge-heading"
      className="overflow-hidden"
    >
      {isLoading || (error && channels === undefined) || !channels?.length ? (
        <>
          <CardHeader className="p-5">
            <CardTitle id="wallet-recharge-heading">
              {t("wallet.rechargeHeading")}
            </CardTitle>
            <CardDescription className="leading-relaxed">
              {t("wallet.rechargeDescription")}
            </CardDescription>
          </CardHeader>
          <CardContent className="px-5 pb-5">
            {isLoading ? (
              <RechargeDeskSkeleton />
            ) : error && channels === undefined ? (
              <WalletFeedback onRetry={mutate} />
            ) : (
              <EmptyState
                variant="inline"
                className="px-0 py-3"
                icon={<Landmark className="size-6" aria-hidden="true" />}
                title={t("wallet.noChannelsTitle")}
                description={t("wallet.noChannelsDescription")}
                action={
                  isAdmin ? (
                    <Button asChild variant="outline" className="h-11">
                      <Link to="/dashboard/payments">
                        {t("wallet.configurePayments")}
                      </Link>
                    </Button>
                  ) : undefined
                }
              />
            )}
          </CardContent>
        </>
      ) : (
        <form
          onSubmit={handleSubmit}
          className="grid lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]"
        >
          <div className="min-w-0">
            <CardHeader className="gap-2 space-y-0 p-5">
              <CardTitle id="wallet-recharge-heading">
                {t("wallet.rechargeHeading")}
              </CardTitle>
              <CardDescription className="leading-relaxed">
                {t("wallet.rechargeDescription")}
              </CardDescription>
            </CardHeader>
            <CardContent className="px-5 pb-5">
              <FieldGroup className="grid gap-4 sm:grid-cols-2">
                <Field className="sm:col-span-2">
                  <FieldLabel id="recharge-presets-label">
                    {t("wallet.amountUsd")}
                  </FieldLabel>
                  <ToggleGroup
                    type="single"
                    value={PRESET_AMOUNTS.includes(amount) ? amount : ""}
                    onValueChange={(value) => value && setAmount(value)}
                    variant="outline"
                    disabled={submitting}
                    className="grid grid-cols-5 gap-2"
                    aria-labelledby="recharge-presets-label"
                  >
                    {PRESET_AMOUNTS.map((preset) => {
                      const nano = parseUsdToNano(preset);
                      return (
                        <ToggleGroupItem
                          key={preset}
                          value={preset}
                          disabled={
                            submitting ||
                            !bounds ||
                            nano === null ||
                            nano < bounds.min ||
                            nano > bounds.max
                          }
                          className="h-11 w-full px-1 tabular-nums data-[state=on]:border-primary data-[state=on]:bg-primary/5 data-[state=on]:text-foreground"
                        >
                          ${preset}
                        </ToggleGroupItem>
                      );
                    })}
                  </ToggleGroup>
                </Field>
                <Field data-invalid={invalidAmount || undefined}>
                  <FieldLabel htmlFor="recharge-amount">
                    {t("wallet.customAmount")}
                  </FieldLabel>
                  <Input
                    id="recharge-amount"
                    className="h-11 text-base tabular-nums"
                    inputMode="decimal"
                    disabled={submitting}
                    value={amount}
                    onChange={(event) => setAmount(event.target.value)}
                    placeholder={t("wallet.amountUsd")}
                    aria-invalid={invalidAmount}
                    aria-describedby={
                      invalidAmount
                        ? "recharge-amount-range recharge-amount-error"
                        : "recharge-amount-range"
                    }
                  />
                  <FieldDescription id="recharge-amount-range">
                    {t("wallet.amountRange", {
                      min: channel?.min_credit_usd,
                      max: channel?.max_credit_usd,
                    })}
                  </FieldDescription>
                  {invalidAmount ? (
                    <p
                      id="recharge-amount-error"
                      className="text-sm text-error-foreground"
                      role="alert"
                    >
                      {validationMessage}
                    </p>
                  ) : null}
                </Field>
                <Field>
                  <FieldLabel htmlFor="recharge-channel">
                    {t("wallet.channel")}
                  </FieldLabel>
                  <Select
                    value={channel?.id ?? ""}
                    onValueChange={setChannelId}
                    disabled={submitting}
                  >
                    <SelectTrigger
                      id="recharge-channel"
                      className="h-11 min-w-0 text-base"
                    >
                      <SelectValue
                        placeholder={t("wallet.channelPlaceholder")}
                      />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectGroup>
                        {channels.map((candidate) => (
                          <SelectItem key={candidate.id} value={candidate.id}>
                            {candidate.name}
                          </SelectItem>
                        ))}
                      </SelectGroup>
                    </SelectContent>
                  </Select>
                </Field>
              </FieldGroup>
            </CardContent>
          </div>
          <section
            aria-labelledby="wallet-payment-summary"
            className="flex min-w-0 flex-col gap-4 border-t bg-muted/35 p-5 lg:border-l lg:border-t-0"
          >
            <h2 id="wallet-payment-summary" className="text-base font-semibold">
              {t("wallet.paymentSummary")}
            </h2>
            <dl className="flex flex-col gap-4 text-sm">
              <div className="flex flex-wrap items-baseline justify-between gap-2">
                <dt className="text-muted-foreground">
                  {t("wallet.creditAmount")}
                </dt>
                <dd className="min-w-0 break-all font-medium tabular-nums">
                  {inRange && amountNano !== null
                    ? `${formatNanoUsd(amountNano, Math.max(2, (amount.trim().split(".")[1] ?? "").replace(/0+$/, "").length))} USD`
                    : "—"}
                </dd>
              </div>
              <div className="flex flex-wrap items-baseline justify-between gap-2">
                <dt className="text-muted-foreground">
                  {t("wallet.exchangeRate")}
                </dt>
                <dd className="min-w-0 break-all tabular-nums">
                  1 USD = {channel?.usd_rate} {channel?.currency}
                </dd>
              </div>
            </dl>
            <Separator />
            <div
              className="flex flex-col gap-3"
              aria-live="polite"
              aria-atomic="true"
            >
              <span className="text-sm text-muted-foreground">
                {t("wallet.youPay")}
              </span>
              <AnimatePresence mode="wait" initial={false}>
                <motion.div
                  key={`${preview}-${channel?.currency}`}
                  initial={reduced ? { opacity: 0 } : { opacity: 0, y: 10 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={reduced ? { opacity: 0 } : { opacity: 0, y: -10 }}
                  transition={reduced ? { duration: 0 } : springs.snappy}
                  className="flex flex-wrap items-baseline gap-2"
                >
                  <span className="min-w-0 break-all text-3xl font-semibold tracking-tight tabular-nums">
                    {preview ?? "—"}
                  </span>
                  <span className="text-sm text-muted-foreground">
                    {channel?.currency}
                  </span>
                </motion.div>
              </AnimatePresence>
            </div>
            <div className="flex flex-col gap-3">
              <Button
                type="submit"
                variant="primary"
                className="h-11 w-full bg-wallet-action text-wallet-action-foreground hover:bg-wallet-action/90"
                disabled={!channel || !inRange || submitting}
              >
                {submitting ? (
                  <LoaderCircle
                    data-icon="inline-start"
                    className="animate-spin"
                    aria-hidden="true"
                  />
                ) : null}
                {submitting
                  ? t("wallet.redirecting")
                  : t("wallet.rechargeSubmit")}
                {!submitting ? (
                  <ArrowUpRight data-icon="inline-end" aria-hidden="true" />
                ) : null}
              </Button>
              <p className="text-sm leading-relaxed text-muted-foreground">
                {t("wallet.paymentHelp")}
              </p>
            </div>
          </section>
        </form>
      )}
    </Card>
  );
}
