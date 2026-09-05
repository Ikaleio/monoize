import { useTranslation } from "react-i18next";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

export interface QueryErrorProps {
  onRetry: () => unknown;
  retrying?: boolean;
  stale?: boolean;
}

export function QueryError({ onRetry, retrying, stale }: QueryErrorProps) {
  const { t } = useTranslation();
  return (
    <Alert variant="destructive">
      <AlertDescription className="flex flex-wrap items-center justify-between gap-3">
        <span className="min-w-0 flex-1 basis-40">
          {t(stale ? "common.refreshFailed" : "common.loadFailed")}
        </span>
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
      </AlertDescription>
    </Alert>
  );
}
