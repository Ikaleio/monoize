import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { Button } from "@/components/ui/button";
import { AnimatedButton } from "@/components/ui/motion";

interface PaginationFooterProps {
  total: number;
  pageSize: number;
  offset: number;
  onOffsetChange: (offset: number) => void;
}

/** Shared limit/offset pager for the wallet and payments tables (RC-W6). */
export function PaginationFooter({
  total,
  pageSize,
  offset,
  onOffsetChange,
}: PaginationFooterProps) {
  const { t } = useTranslation();
  const pages = Math.max(1, Math.ceil(total / pageSize));
  const page = Math.floor(offset / pageSize) + 1;
  if (pages <= 1) return null;

  return (
    <div className="flex items-center justify-between gap-2 pt-3">
      <span className="text-xs text-muted-foreground tabular-nums">
        {t("wallet.page", { page, pages })}
      </span>
      <div className="flex items-center gap-1">
        <AnimatedButton>
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="size-8"
            aria-label={t("wallet.previousPage")}
            disabled={offset === 0}
            onClick={() => onOffsetChange(Math.max(0, offset - pageSize))}
          >
            <ChevronLeft aria-hidden="true" />
          </Button>
        </AnimatedButton>
        <AnimatedButton>
          <Button
            type="button"
            variant="outline"
            size="icon"
            className="size-8"
            aria-label={t("wallet.nextPage")}
            disabled={page >= pages}
            onClick={() => onOffsetChange(offset + pageSize)}
          >
            <ChevronRight aria-hidden="true" />
          </Button>
        </AnimatedButton>
      </div>
    </div>
  );
}
