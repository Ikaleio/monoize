import { useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { BookOpenText } from "lucide-react";
import {
  DataList,
  DataListBody,
  DataListCell,
  DataListHead,
  DataListHeader,
  DataListRow,
} from "@/components/ui/data-list";
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
      className="flex flex-col"
    >
      <div className="flex justify-end p-4">
        <Select
          value={kind}
          onValueChange={(value) => {
            setKind(value);
            setOffset(0);
          }}
        >
          <SelectTrigger
            className="h-11 w-full sm:w-64"
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
        <div className="flex flex-col gap-2 px-4 pb-4" aria-busy="true">
          {Array.from({ length: 5 }).map((_, index) => (
            <Skeleton key={index} className="h-14 w-full" />
          ))}
        </div>
      ) : error && !data ? (
        <div className="px-4 pb-4">
          <WalletFeedback onRetry={mutate} />
        </div>
      ) : !data?.entries.length ? (
        <EmptyState
          variant="inline"
          className="px-4 py-7"
          icon={<BookOpenText className="size-6" aria-hidden="true" />}
          title={t("wallet.noLedger")}
        />
      ) : (
        <>
          <DataList columns="minmax(0,1.4fr) 9rem 9rem 10rem" className="border-t">
            <DataListHeader>
              <DataListHead>{t("wallet.kind")}</DataListHead>
              <DataListHead align="end">{t("wallet.delta")}</DataListHead>
              <DataListHead align="end">{t("wallet.balanceAfter")}</DataListHead>
              <DataListHead>{t("wallet.createdAt")}</DataListHead>
            </DataListHeader>
            <DataListBody aria-label={t("wallet.ledgerTitle")}>
              {data.entries.map((entry, index) => {
                const positive = !entry.delta_nano_usd.startsWith("-");
                return (
                  <DataListRow key={entry.id} asChild>
                    <motion.li
                      layout={!reduced}
                      initial={reduced ? { opacity: 0 } : { opacity: 0, y: 12 }}
                      animate={{ opacity: 1, y: 0 }}
                      transition={
                        reduced
                          ? { duration: 0 }
                          : { ...springs.gentle, delay: Math.min(index * 0.03, 0.18) }
                      }
                    >
                      <DataListCell primary>
                        <span className="font-medium">{kindLabel(entry.kind)}</span>
                      </DataListCell>
                      <DataListCell label={t("wallet.delta")} align="end">
                        <span
                          className={cn(
                            "font-medium tabular-nums",
                            positive ? "text-success" : "text-error-foreground",
                          )}
                        >
                          {positive ? "+" : ""}
                          {formatNanoUsd(entry.delta_nano_usd, 4)}
                        </span>
                      </DataListCell>
                      <DataListCell label={t("wallet.balanceAfter")} align="end">
                        <span className="tabular-nums">
                          {entry.balance_after_nano_usd !== null
                            ? formatNanoUsd(entry.balance_after_nano_usd, 4)
                            : "—"}
                        </span>
                      </DataListCell>
                      <DataListCell label={t("wallet.createdAt")}>
                        <span className="tabular-nums text-muted-foreground">
                          {formatTime(entry.created_at)}
                        </span>
                      </DataListCell>
                    </motion.li>
                  </DataListRow>
                );
              })}
            </DataListBody>
          </DataList>
          <div className="border-t px-4 pb-3 empty:hidden">
            <PaginationFooter
              total={data.total}
              pageSize={PAGE_SIZE}
              offset={offset}
              onOffsetChange={setOffset}
            />
          </div>
        </>
      )}
    </motion.section>
  );
}
