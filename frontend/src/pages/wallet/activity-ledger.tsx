import { useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { ArrowDownLeft, ArrowUpRight, BookOpenText, CalendarClock } from "lucide-react";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Skeleton } from "@/components/ui/skeleton";
import { springs } from "@/components/ui/motion";
import { formatTime } from "@/pages/request-logs/utils";
import { formatNanoUsd } from "@/lib/exact-decimal";
import { WALLET_LEDGER_KINDS } from "@/lib/recharge";
import { useLedger } from "@/lib/swr";
import { cn } from "@/lib/utils";
import { PaginationFooter } from "./pagination-footer";
import { WalletFeedback } from "./wallet-feedback";

const PAGE_SIZE = 10;
const ALL_KINDS = [...WALLET_LEDGER_KINDS];

export function ActivityLedger({
  active,
  username,
}: {
  active: boolean;
  username: string;
}) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const [kind, setKind] = useState("all");
  const [offset, setOffset] = useState(0);
  const kinds = kind === "all" ? ALL_KINDS : [kind];
  const { data, error, isLoading, mutate } = useLedger(
    PAGE_SIZE,
    offset,
    kinds,
    username,
  );
  const kindLabel = (value: string) =>
    t(`wallet.kinds.${value}`, { defaultValue: value });

  return (
    <motion.section
      aria-label={t("wallet.ledgerTitle")}
      initial={false}
      animate={active || reduced ? { opacity: 1, x: 0 } : { opacity: 0, x: 20 }}
      transition={reduced ? { duration: 0 } : springs.gentle}
      className="flex flex-col gap-4 p-5"
    >
      <div className="flex justify-end">
        <Select
          value={kind}
          onValueChange={(value) => {
            setKind(value);
            setOffset(0);
          }}
        >
          <SelectTrigger
            className="h-11 w-full sm:h-9 sm:w-64"
            aria-label={t("wallet.kind")}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem value="all">{t("wallet.kindFilterAll")}</SelectItem>
              {ALL_KINDS.map((value) => (
                <SelectItem key={value} value={value}>
                  {kindLabel(value)}
                </SelectItem>
              ))}
            </SelectGroup>
          </SelectContent>
        </Select>
      </div>

      {isLoading ? (
        <div className="flex flex-col gap-2" aria-busy="true">
          {Array.from({ length: 5 }).map((_, index) => (
            <Skeleton key={index} className="h-20 w-full md:h-14" />
          ))}
        </div>
      ) : error && !data ? (
        <WalletFeedback onRetry={mutate} />
      ) : !data?.entries.length ? (
        <EmptyState
          variant="inline"
          className="px-4 py-7"
          icon={<BookOpenText className="size-6" aria-hidden="true" />}
          title={t("wallet.noLedger")}
        />
      ) : (
        <div className="flex flex-col gap-3">
          <div className="hidden grid-cols-[minmax(0,1.4fr)_minmax(8rem,0.6fr)_minmax(9rem,0.7fr)_minmax(10rem,0.75fr)] gap-4 border-b px-3 pb-2 text-xs font-medium text-muted-foreground md:grid">
            <span>{t("wallet.kind")}</span>
            <span className="text-right">{t("wallet.delta")}</span>
            <span className="text-right">{t("wallet.balanceAfter")}</span>
            <span>{t("wallet.createdAt")}</span>
          </div>

          <motion.ul layout={!reduced} className="divide-y">
            {data.entries.map((entry, index) => {
              const positive = !entry.delta_nano_usd.startsWith("-");
              const EntryIcon = positive ? ArrowDownLeft : ArrowUpRight;

              return (
                <motion.li
                  key={entry.id}
                  layout={!reduced}
                  initial={reduced ? { opacity: 0 } : { opacity: 0, y: 12 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={
                    reduced
                      ? { duration: 0 }
                      : { ...springs.gentle, delay: Math.min(index * 0.03, 0.18) }
                  }
                  className="grid gap-3 rounded-md px-3 py-3 transition-colors hover:bg-muted/50 md:grid-cols-[minmax(0,1.4fr)_minmax(8rem,0.6fr)_minmax(9rem,0.7fr)_minmax(10rem,0.75fr)] md:items-center md:gap-4"
                >
                  <div className="flex min-w-0 items-center gap-3">
                    <span
                      className={cn(
                        "flex size-9 shrink-0 items-center justify-center rounded-md",
                        positive
                          ? "bg-success-soft text-success-foreground"
                          : "bg-destructive/10 text-destructive",
                      )}
                    >
                      <EntryIcon className="size-4" aria-hidden="true" />
                    </span>
                    <span className="min-w-0 text-pretty text-sm font-medium">
                      {kindLabel(entry.kind)}
                    </span>
                  </div>

                  <div className="flex items-baseline justify-between gap-3 md:block md:text-right">
                    <span className="text-sm text-muted-foreground md:hidden">
                      {t("wallet.delta")}
                    </span>
                    <span
                      className={cn(
                        "font-display text-lg font-semibold tabular-nums",
                        positive ? "text-success" : "text-destructive",
                      )}
                    >
                      {positive ? "+" : ""}
                      {formatNanoUsd(entry.delta_nano_usd, 4)}
                    </span>
                  </div>

                  <div className="flex items-baseline justify-between gap-3 md:block md:text-right">
                    <span className="text-sm text-muted-foreground md:hidden">
                      {t("wallet.balanceAfter")}
                    </span>
                    <span className="text-sm tabular-nums">
                      {entry.balance_after_nano_usd !== null
                        ? formatNanoUsd(entry.balance_after_nano_usd, 4)
                        : "—"}
                    </span>
                  </div>

                  <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground tabular-nums">
                    <CalendarClock className="size-4" aria-hidden="true" />
                    {formatTime(entry.created_at)}
                  </span>
                </motion.li>
              );
            })}
          </motion.ul>

          <PaginationFooter
            total={data.total}
            pageSize={PAGE_SIZE}
            offset={offset}
            onOffsetChange={setOffset}
          />
        </div>
      )}
    </motion.section>
  );
}
