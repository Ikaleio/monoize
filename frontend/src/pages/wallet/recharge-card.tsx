import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ArrowRight, CircleDollarSign, LoaderCircle } from "lucide-react";
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
import { Skeleton } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { AnimatedButton, springs } from "@/components/ui/motion";
import { DashboardApiError } from "@/lib/api";
import { parseUsdToNano, previewPayAmount } from "@/lib/recharge";
import { createRechargeOrderOptimistic, useRechargeChannels } from "@/lib/swr";
import { WalletSectionError } from "./section-error";

const PRESET_AMOUNTS = ["5", "10", "25", "50", "100"];

interface RechargeCardProps {
  /** SWR key of the first orders page, for the RC-W3 optimistic insert. */
  ordersFirstPageKey: string;
}

function RechargeSkeleton() {
  return (
    <div className="flex flex-col gap-5" aria-busy="true">
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
  const {
    data: channels,
    error,
    isLoading,
    mutate,
  } = useRechargeChannels();
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
          ? t(`wallet.errors.${error.code}`, {
              defaultValue: t("wallet.errors.request_failed"),
            })
          : t("wallet.errors.request_failed");
      toast.error(message);
    }
  };

  return (
    <Card
      role="region"
      className="lg:col-span-7"
      aria-labelledby="wallet-recharge-title"
    >
      <CardHeader className="flex flex-row items-start gap-3 p-5 pb-0 sm:p-6 sm:pb-0">
        <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary text-primary-foreground">
          <CircleDollarSign className="size-5" aria-hidden="true" />
        </span>
        <div className="flex min-w-0 flex-col gap-1">
          <CardTitle
            id="wallet-recharge-title"
            className="text-balance font-display text-lg"
          >
            {t("wallet.rechargeTitle")}
          </CardTitle>
          <CardDescription className="text-pretty">
            {t("wallet.rechargeDescription")}
          </CardDescription>
        </div>
      </CardHeader>

      <CardContent className="p-5 pt-5 sm:p-6 sm:pt-5">
        {isLoading ? (
          <RechargeSkeleton />
        ) : error && channels === undefined ? (
          <WalletSectionError onRetry={mutate} />
        ) : !channels || channels.length === 0 ? (
          <EmptyState
            variant="inline"
            className="px-4 py-6"
            icon={<CircleDollarSign className="size-6" aria-hidden="true" />}
            title={t("wallet.noChannelsTitle")}
            description={t("wallet.noChannelsDescription")}
          />
        ) : (
          <motion.div
            initial={reduced ? { opacity: 0 } : { opacity: 0, x: 24 }}
            animate={{ opacity: 1, x: 0 }}
            transition={reduced ? { duration: 0 } : springs.gentle}
            className="flex flex-col gap-5"
          >
            <FieldGroup className="grid gap-5 sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1.2fr)]">
              <Field>
                <FieldLabel htmlFor="recharge-channel">
                  {t("wallet.channel")}
                </FieldLabel>
                <Select
                  value={channel?.id ?? ""}
                  onValueChange={(value) => setChannelId(value)}
                >
                  <SelectTrigger id="recharge-channel" className="h-11 sm:h-9">
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
                  size="default"
                  className="grid grid-cols-5"
                  aria-label={t("wallet.amountUsd")}
                >
                  {PRESET_AMOUNTS.map((preset) => (
                    <ToggleGroupItem
                      key={preset}
                      value={preset}
                      disabled={presetDisabled(preset)}
                      className="relative isolate h-11 w-full overflow-hidden px-1 tabular-nums data-[state=on]:bg-transparent sm:h-10"
                    >
                      {selectedPreset === preset ? (
                        <motion.span
                          layoutId={
                            reduced ? undefined : "wallet-amount-selection"
                          }
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
                  className="h-11 sm:h-9"
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

            <motion.div
              layout={!reduced}
              className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto]"
            >
              <div className="flex min-h-14 items-center justify-between gap-4 rounded-lg bg-muted px-4 py-3">
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

              <AnimatedButton className="w-full sm:w-auto">
                <Button
                  type="button"
                  variant="primary"
                  size="lg"
                  className="h-12 w-full sm:h-14 sm:w-auto"
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
      </CardContent>
    </Card>
  );
}
