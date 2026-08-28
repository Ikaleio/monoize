import { useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { BookOpenText } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { springs } from "@/components/ui/motion";
import { PaginationFooter } from "./pagination-footer";
import { formatTime } from "@/pages/request-logs/utils";
import { formatNanoUsd } from "@/lib/exact-decimal";
import { WALLET_LEDGER_KINDS } from "@/lib/recharge";
import { useLedger } from "@/lib/swr";
import { cn } from "@/lib/utils";

const PAGE_SIZE = 10;
const ALL_KINDS = [...WALLET_LEDGER_KINDS];

/**
 * RC-W5: wallet-level ledger entries. The default `kinds` filter is exactly
 * the non-per-request kind list; per-request charges stay on /dashboard/logs.
 */
export function LedgerSection({ username }: { username: string }) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const [kind, setKind] = useState<string>("all");
  const [offset, setOffset] = useState(0);

  const kinds = kind === "all" ? ALL_KINDS : [kind];
  const { data, isLoading } = useLedger(PAGE_SIZE, offset, kinds, username);

  const kindLabel = (value: string) =>
    t(`wallet.kinds.${value}`, { defaultValue: value });

  return (
    <Card>
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2 pb-2">
        <CardTitle className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          <BookOpenText className="h-4 w-4" />
          {t("wallet.ledgerTitle")}
        </CardTitle>
        <Select
          value={kind}
          onValueChange={(value) => {
            setKind(value);
            setOffset(0);
          }}
        >
          <SelectTrigger className="h-8 w-52" aria-label={t("wallet.kind")}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">{t("wallet.kindFilterAll")}</SelectItem>
            {ALL_KINDS.map((value) => (
              <SelectItem key={value} value={value}>
                {kindLabel(value)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </CardHeader>
      <CardContent>
        {isLoading ? (
          <div className="flex flex-col gap-3" aria-busy="true">
            {Array.from({ length: 4 }).map((_, index) => (
              <Skeleton key={index} className="h-9 w-full" />
            ))}
          </div>
        ) : !data || data.entries.length === 0 ? (
          <EmptyState variant="inline" title={t("wallet.noLedger")} />
        ) : (
          <motion.div
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            transition={{ duration: reduced ? 0 : 0.25 }}
            className="overflow-x-auto"
          >
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr className="border-b">
                  <th className="px-3 py-2 font-medium">{t("common.created")}</th>
                  <th className="px-3 py-2 font-medium">{t("wallet.kind")}</th>
                  <th className="px-3 py-2 font-medium">{t("wallet.delta")}</th>
                  <th className="px-3 py-2 font-medium">{t("wallet.balanceAfter")}</th>
                </tr>
              </thead>
              <tbody>
                {data.entries.map((entry) => {
                  const positive = !entry.delta_nano_usd.startsWith("-");
                  return (
                    <motion.tr
                      key={entry.id}
                      layout={!reduced}
                      initial={reduced ? { opacity: 0 } : { opacity: 0, y: -8 }}
                      animate={{ opacity: 1, y: 0 }}
                      transition={reduced ? { duration: 0.15 } : springs.smooth}
                      className="border-b transition-colors last:border-b-0 hover:bg-accent/40"
                    >
                      <td className="whitespace-nowrap px-3 py-2.5 tabular-nums text-muted-foreground">
                        {formatTime(entry.created_at)}
                      </td>
                      <td className="px-3 py-2.5">{kindLabel(entry.kind)}</td>
                      <td
                        className={cn(
                          "whitespace-nowrap px-3 py-2.5 font-medium tabular-nums",
                          positive ? "text-success" : "text-destructive",
                        )}
                      >
                        {positive ? "+" : ""}
                        {formatNanoUsd(entry.delta_nano_usd, 4)}
                      </td>
                      <td className="whitespace-nowrap px-3 py-2.5 tabular-nums text-muted-foreground">
                        {entry.balance_after_nano_usd !== null
                          ? formatNanoUsd(entry.balance_after_nano_usd, 4)
                          : "—"}
                      </td>
                    </motion.tr>
                  );
                })}
              </tbody>
            </table>
            <PaginationFooter
              total={data.total}
              pageSize={PAGE_SIZE}
              offset={offset}
              onOffsetChange={setOffset}
            />
          </motion.div>
        )}
      </CardContent>
    </Card>
  );
}
