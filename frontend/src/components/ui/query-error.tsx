import { useTranslation } from "react-i18next";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

export interface QueryErrorProps {
  onRetry: () => unknown;
  retrying?: boolean;
  stale?: boolean;
  /** The request was rejected for lack of permission; retrying cannot succeed. */
  forbidden?: boolean;
}

export function QueryError({ onRetry, retrying, stale, forbidden }: QueryErrorProps) {
  const { t } = useTranslation();
  return (
    <Alert variant="destructive">
      <AlertDescription className="flex flex-wrap items-center justify-between gap-3">
        <span className="min-w-0 flex-1 basis-40">
          {t(
            forbidden
              ? "common.forbidden"
              : stale
                ? "common.refreshFailed"
                : "common.loadFailed",
          )}
        </span>
        {forbidden ? null : (
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={retrying}
            onClick={async () => {
              try {
                await onRetry();
              } catch {
                // SWR exposes retry failures through the query's error state.
              }
            }}
          >
            {t(retrying ? "common.loading" : "common.retry")}
          </Button>
        )}
      </AlertDescription>
    </Alert>
  );
}
