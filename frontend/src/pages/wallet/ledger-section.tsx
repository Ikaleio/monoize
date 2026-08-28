import { useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { ArrowDownLeft, ArrowUpRight, BookOpenText } from "lucide-react";
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
import { PaginationFooter } from "./pagination-footer";
import { formatTime } from "@/pages/request-logs/utils";
import { formatNanoUsd } from "@/lib/exact-decimal";
import { WALLET_LEDGER_KINDS } from "@/lib/recharge";
import { useLedger } from "@/lib/swr";
import { cn } from "@/lib/utils";

const PAGE_SIZE = 10;
const ALL_KINDS = [...WALLET_LEDGER_KINDS];

/** RC-W5: wallet-level movements; per-request charges stay on request logs. */
export function LedgerSection({
  active,
  username,
}: {
  active: boolean;
  username: string;
}) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const [kind, setKind] = useState<string>("all");
  const [offset, setOffset] = useState(0);

  const kinds = kind === "all" ? ALL_KINDS : [kind];
  const { data, isLoading } = useLedger(PAGE_SIZE, offset, kinds, username);
  const kindLabel = (value: string) =>
    t(`wallet.kinds.${value}`, { defaultValue: value });

  return (
    <motion.section
      aria-label={t("wallet.ledgerTitle")}
      initial={false}
      animate={active || reduced ? { opacity: 1, x: 0 } : { opacity: 0, x: 24 }}
      transition={reduced ? { duration: 0 } : springs.gentle}
      className="flex flex-col gap-4"
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
            className="w-full sm:w-64"
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
        <div className="flex flex-col gap-3" aria-busy="true">
          {Array.from({ length: 4 }).map((_, index) => (
            <Skeleton key={index} className="h-20 w-full" />
          ))}
        </div>
      ) : !data || data.entries.length === 0 ? (
        <EmptyState
          variant="inline"
          icon={<BookOpenText className="size-6" aria-hidden="true" />}
          title={t("wallet.noLedger")}
        />
      ) : (
        <div className="flex flex-col gap-3">
          <motion.ul layout={!reduced} className="flex flex-col gap-1">
            {data.entries.map((entry, index) => {
              const positive = !entry.delta_nano_usd.startsWith("-");
              const EntryIcon = positive ? ArrowDownLeft : ArrowUpRight;

              return (
                <motion.li
                  key={entry.id}
                  layout={!reduced}
                  initial={reduced ? { opacity: 0 } : { opacity: 0, y: 16 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={
                    reduced
                      ? { duration: 0 }
                      : {
                          ...springs.gentle,
                          delay: Math.min(index * 0.035, 0.2),
                        }
                  }
                  className="flex items-start justify-between gap-4 rounded-lg px-3 py-4 transition-colors hover:bg-muted/50 sm:px-4"
                >
                  <div className="flex min-w-0 items-start gap-3">
                    <span
                      className={cn(
                        "flex size-9 shrink-0 items-center justify-center rounded-full",
                        positive
                          ? "bg-success-soft text-success-foreground"
                          : "bg-destructive/10 text-destructive",
                      )}
                    >
                      <EntryIcon aria-hidden="true" />
                    </span>
                    <div className="flex min-w-0 flex-col gap-1">
                      <span className="text-pretty font-medium">
                        {kindLabel(entry.kind)}
                      </span>
                      <span className="text-xs text-muted-foreground tabular-nums">
                        {formatTime(entry.created_at)}
                      </span>
                    </div>
                  </div>
                  <div className="flex shrink-0 flex-col items-end gap-1">
                    <span
                      className={cn(
                        "font-display text-lg font-semibold tabular-nums",
                        positive ? "text-success" : "text-destructive",
                      )}
                    >
                      {positive ? "+" : ""}
                      {formatNanoUsd(entry.delta_nano_usd, 4)}
                    </span>
                    <span className="text-xs text-muted-foreground tabular-nums">
                      {t("wallet.balanceAfter")}:{" "}
                      {entry.balance_after_nano_usd !== null
                        ? formatNanoUsd(entry.balance_after_nano_usd, 4)
                        : "—"}
                    </span>
                  </div>
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
