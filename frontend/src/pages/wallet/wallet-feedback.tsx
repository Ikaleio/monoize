import { useTranslation } from "react-i18next";
import { RefreshCw, TriangleAlert } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

export function WalletFeedback({ onRetry }: { onRetry: () => unknown }) {
  const { t } = useTranslation();

  return (
    <Alert className="bg-muted/40">
      <TriangleAlert aria-hidden="true" />
      <AlertTitle>{t("wallet.loadErrorTitle")}</AlertTitle>
      <AlertDescription className="flex flex-col items-start gap-3">
        <span>{t("wallet.loadErrorDescription")}</span>
        <Button type="button" variant="outline" size="sm" onClick={onRetry}>
          <RefreshCw data-icon="inline-start" aria-hidden="true" />
          {t("wallet.retry")}
        </Button>
      </AlertDescription>
    </Alert>
  );
}
