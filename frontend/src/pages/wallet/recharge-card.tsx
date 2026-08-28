import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ArrowRight, CircleDollarSign, LoaderCircle } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
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
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { AnimatedButton, springs } from "@/components/ui/motion";
import { DashboardApiError } from "@/lib/api";
import { parseUsdToNano, previewPayAmount } from "@/lib/recharge";
import { createRechargeOrderOptimistic, useRechargeChannels } from "@/lib/swr";

const PRESET_AMOUNTS = ["5", "10", "25", "50", "100"];

interface RechargeCardProps {
  /** SWR key of the first orders page, for the RC-W3 optimistic insert. */
  ordersFirstPageKey: string;
}

function RechargeSkeleton() {
  return (
    <div
      className="flex flex-col gap-5"
      aria-busy="true"
    >
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="h-9 w-full" />
      </div>
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-20" />
        <Skeleton className="h-9 w-full" />
        <Skeleton className="h-9 w-full" />
      </div>
      <Skeleton className="h-20 w-full" />
    </div>
  );
}

/** RC-W3: exact recharge preview and optimistic pending-order creation. */
export function RechargeCard({ ordersFirstPageKey }: RechargeCardProps) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const { data: channels, isLoading } = useRechargeChannels();
  const [channelId, setChannelId] = useState<string | null>(null);
  const [amount, setAmount] = useState("10");
  const [submitting, setSubmitting] = useState(false);

  const channel = useMemo(() => {
    if (!channels || channels.length === 0) return null;
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
  const selectedPreset = PRESET_AMOUNTS.includes(amount) ? amount : "";

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
      window.location.assign(result.payment.url);
    } catch (error) {
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
    <section
      className="flex flex-col gap-6 p-6 sm:p-8 lg:p-10"
      aria-labelledby="wallet-recharge-title"
    >
      <header className="flex items-center gap-3">
        <span className="flex size-10 items-center justify-center rounded-full bg-primary text-primary-foreground">
          <CircleDollarSign className="size-5" aria-hidden="true" />
        </span>
        <div className="flex flex-col gap-1">
          <h2
            id="wallet-recharge-title"
            className="font-display text-xl font-semibold tracking-tight"
          >
            {t("wallet.rechargeTitle")}
          </h2>
          <p className="text-sm text-muted-foreground">
            {t("wallet.rechargeDescription")}
          </p>
        </div>
      </header>

      {isLoading ? (
        <RechargeSkeleton />
      ) : !channels || channels.length === 0 ? (
        <EmptyState
          variant="inline"
          icon={<CircleDollarSign className="size-6" aria-hidden="true" />}
          title={t("wallet.noChannelsTitle")}
          description={t("wallet.noChannelsDescription")}
        />
      ) : (
        <motion.div
          initial={reduced ? { opacity: 0 } : { opacity: 0, x: 24 }}
          animate={{ opacity: 1, x: 0 }}
          transition={reduced ? { duration: 0 } : springs.gentle}
          className="flex flex-1 flex-col gap-6"
        >
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="recharge-channel">
                {t("wallet.channel")}
              </FieldLabel>
              <Select
                value={channel?.id ?? ""}
                onValueChange={(value) => setChannelId(value)}
              >
                <SelectTrigger id="recharge-channel">
                  <SelectValue placeholder={t("wallet.channelPlaceholder")} />
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

            <Field data-invalid={amount !== "" && !inRange ? true : undefined}>
              <FieldLabel htmlFor="recharge-amount">
                {t("wallet.amountUsd")}
              </FieldLabel>
              <ToggleGroup
                type="single"
                value={selectedPreset}
                onValueChange={(value) => {
                  if (value) setAmount(value);
                }}
                variant="outline"
                size="sm"
                className="grid grid-cols-5"
                aria-label={t("wallet.amountUsd")}
              >
                {PRESET_AMOUNTS.map((preset) => (
                  <ToggleGroupItem
                    key={preset}
                    value={preset}
                    disabled={presetDisabled(preset)}
                    className="relative isolate w-full overflow-hidden px-1 tabular-nums data-[state=on]:bg-transparent"
                  >
                    {selectedPreset === preset ? (
                      <motion.span
                        layoutId={reduced ? undefined : "wallet-amount-selection"}
                        transition={reduced ? { duration: 0 } : springs.snappy}
                        className="absolute inset-0 -z-10 rounded-md bg-accent"
                      />
                    ) : null}
                    <span>${preset}</span>
                  </ToggleGroupItem>
                ))}
              </ToggleGroup>
              <Input
                id="recharge-amount"
                inputMode="decimal"
                value={amount}
                onChange={(event) => setAmount(event.target.value)}
                placeholder={t("wallet.customAmount")}
                aria-invalid={amount !== "" && !inRange}
              />
              {channel ? (
                <FieldDescription>
                  {t("wallet.amountRange", {
                    min: channel.min_credit_usd,
                    max: channel.max_credit_usd,
                  })}
                </FieldDescription>
              ) : null}
            </Field>
          </FieldGroup>

          <motion.div layout={!reduced} className="flex flex-col gap-4">
            <div className="flex min-h-20 items-center justify-between gap-4 rounded-lg bg-muted p-4">
              <span className="text-sm text-muted-foreground">
                {t("wallet.youPay")}
              </span>
              <div className="relative flex min-h-8 min-w-0 flex-1 justify-end overflow-hidden">
                <AnimatePresence mode="popLayout" initial={false}>
                  <motion.span
                    key={preview ? `${preview}-${channel.currency}` : "empty"}
                    initial={
                      reduced
                        ? { opacity: 0 }
                        : { opacity: 0, y: 18, scale: 0.96 }
                    }
                    animate={{ opacity: 1, y: 0, scale: 1 }}
                    exit={
                      reduced
                        ? { opacity: 0 }
                        : { opacity: 0, y: -18, scale: 0.96 }
                    }
                    transition={reduced ? { duration: 0 } : springs.snappy}
                    className="truncate font-display text-xl font-semibold tabular-nums"
                  >
                    {preview ? `${preview} ${channel.currency}` : "—"}
                  </motion.span>
                </AnimatePresence>
              </div>
            </div>

            <AnimatedButton className="w-full">
              <Button
                type="button"
                variant="primary"
                size="lg"
                className="w-full"
                disabled={!channel || !inRange || submitting}
                onClick={handleSubmit}
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
                  <ArrowRight data-icon="inline-end" aria-hidden="true" />
                ) : null}
              </Button>
            </AnimatedButton>
          </motion.div>
        </motion.div>
      )}
    </section>
  );
}
