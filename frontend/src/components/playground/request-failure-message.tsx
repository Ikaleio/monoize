import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { cn } from "@/lib/utils";
import type { PlaygroundRequestFailure } from "./request-failure";

export function RequestFailureMessage({
  failure,
  className,
}: {
  failure: PlaygroundRequestFailure;
  className?: string;
}) {
  const { t } = useTranslation();
  const message = failure.message || t("playground.errorNoDetail");

  return (
    <span className={cn("min-w-0 break-words text-sm text-error-foreground", className)}>
      {failure.status !== null
        ? t("playground.requestFailedStatus", { status: failure.status, message })
        : t("playground.requestFailed", { message })}
      {failure.requestId && (
        <Link
          to={`/dashboard/logs?q=${encodeURIComponent(failure.requestId)}`}
          className="ml-2 whitespace-nowrap underline underline-offset-4 hover:text-primary"
        >
          {t("playground.viewInLogs")}
        </Link>
      )}
    </span>
  );
}
