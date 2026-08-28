import { forwardRef, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { ArrowUpRight, Landmark, LoaderCircle } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
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
import { springs } from "@/components/ui/motion";
import { DashboardApiError } from "@/lib/api";
import { parseUsdToNano, previewPayAmount } from "@/lib/recharge";
import { createRechargeOrderOptimistic, useRechargeChannels } from "@/lib/swr";
import { cn } from "@/lib/utils";
import { WalletFeedback } from "./wallet-feedback";

const PRESET_AMOUNTS = ["5", "10", "25", "50", "100"];

interface RechargeDeskProps {
  className?: string;
  ordersFirstPageKey: string;
}

function RechargeDeskSkeleton() {
  return (
    <div className="flex flex-col gap-5" aria-busy="true">
      <div className="flex flex-col gap-2">
        <Skeleton className="h-4 w-24" />
        <Skeleton className="h-11 w-full" />
      </div>
      <div className="flex flex-col gap-3">
        <Skeleton className="h-4 w-20" />
        <Skeleton className="h-11 w-full" />
        <Skeleton className="h-11 w-full" />
      </div>
      <Skeleton className="h-16 w-full" />
    </div>
  );
}

export const RechargeDesk = forwardRef<HTMLElement, RechargeDeskProps>(
  ({ className, ordersFirstPageKey }, ref) => {
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
      } catch (caught) {
        setSubmitting(false);
        const message =
          caught instanceof DashboardApiError
            ? t(`wallet.errors.${caught.code}`, {
                defaultValue: t("wallet.errors.request_failed"),
              })
            : t("wallet.errors.request_failed");
        toast.error(message);
      }
    };

    const hasChannels = Boolean(channels?.length);

    return (
      <section
        ref={ref}
        tabIndex={-1}
        aria-labelledby="wallet-recharge-heading"
        className={cn("scroll-mt-4 focus:outline-none", className)}
      >
        <Card>
          <CardHeader className="border-b p-5">
            <div className="flex items-start gap-3">
              <span className="flex size-9 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground">
                <Landmark className="size-5" aria-hidden="true" />
              </span>
              <div className="flex min-w-0 flex-col gap-1">
                <CardTitle
                  id="wallet-recharge-heading"
                  className="font-display text-lg"
                >
                  {t("wallet.rechargeTitle")}
                </CardTitle>
                <CardDescription className="text-pretty leading-relaxed">
                  {t("wallet.rechargeDescription")}
                </CardDescription>
              </div>
            </div>
          </CardHeader>

          <CardContent className="p-5">
            {isLoading ? (
              <RechargeDeskSkeleton />
            ) : error && channels === undefined ? (
              <WalletFeedback onRetry={mutate} />
            ) : !hasChannels ? (
              <EmptyState
                variant="inline"
                className="px-2 py-5"
                icon={<Landmark className="size-6" aria-hidden="true" />}
                title={t("wallet.noChannelsTitle")}
                description={t("wallet.noChannelsDescription")}
              />
            ) : (
              <motion.div
                initial={reduced ? { opacity: 0 } : { opacity: 0, y: 16 }}
                animate={{ opacity: 1, y: 0 }}
                transition={reduced ? { duration: 0 } : springs.gentle}
              >
                <FieldGroup className="gap-5">
                  <Field>
                    <FieldLabel htmlFor="recharge-channel">
                      {t("wallet.channel")}
                    </FieldLabel>
                    <Select
                      value={channel?.id ?? ""}
                      onValueChange={setChannelId}
                    >
                      <SelectTrigger id="recharge-channel" className="h-11 sm:h-9">
                        <SelectValue placeholder={t("wallet.channelPlaceholder")} />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectGroup>
                          {channels?.map((candidate) => (
                            <SelectItem key={candidate.id} value={candidate.id}>
                              {candidate.name}
                            </SelectItem>
                          ))}
                        </SelectGroup>
                      </SelectContent>
                    </Select>
                  </Field>

                  <Field data-invalid={amount !== "" && !inRange || undefined}>
                    <FieldLabel htmlFor="recharge-amount">
                      {t("wallet.amountUsd")}
                    </FieldLabel>
                    <ToggleGroup
                      type="single"
                      value={selectedPreset}
                      onValueChange={(value) => value && setAmount(value)}
                      variant="outline"
                      className="grid grid-cols-5"
                      aria-label={t("wallet.amountUsd")}
                    >
                      {PRESET_AMOUNTS.map((preset) => (
                        <ToggleGroupItem
                          key={preset}
                          value={preset}
                          disabled={presetDisabled(preset)}
                          className="h-11 w-full px-1 tabular-nums sm:h-9"
                        >
                          ${preset}
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
              </motion.div>
            )}
          </CardContent>

          {hasChannels ? (
            <CardFooter className="grid gap-4 border-t bg-muted/35 p-5 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
              <div className="flex min-w-0 items-center gap-3">
                <span className="text-sm text-muted-foreground">
                  {t("wallet.youPay")}
                </span>
                <div className="relative min-h-7 min-w-0 flex-1 overflow-hidden text-right">
                  <AnimatePresence mode="popLayout" initial={false}>
                    <motion.span
                      key={preview && channel ? `${preview}-${channel.currency}` : "empty"}
                      initial={reduced ? { opacity: 0 } : { opacity: 0, y: 14 }}
                      animate={{ opacity: 1, y: 0 }}
                      exit={reduced ? { opacity: 0 } : { opacity: 0, y: -14 }}
                      transition={reduced ? { duration: 0 } : springs.snappy}
                      className="block truncate font-display text-lg font-semibold tabular-nums"
                    >
                      {preview && channel
                        ? `${preview} ${channel.currency}`
                        : "—"}
                    </motion.span>
                  </AnimatePresence>
                </div>
              </div>
              <Button
                type="button"
                variant="primary"
                className="h-11 w-full sm:h-9 sm:w-auto"
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
                  <ArrowUpRight data-icon="inline-end" aria-hidden="true" />
                ) : null}
              </Button>
            </CardFooter>
          ) : null}
        </Card>
      </section>
    );
  },
);

RechargeDesk.displayName = "RechargeDesk";
