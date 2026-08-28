import { CircleAlert, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";

interface WalletSectionErrorProps {
  onRetry: () => void | Promise<unknown>;
}

export function WalletSectionError({ onRetry }: WalletSectionErrorProps) {
  const { t } = useTranslation();

  return (
    <EmptyState
      variant="inline"
      className="px-4 py-6"
      icon={<CircleAlert className="size-6" aria-hidden="true" />}
      title={t("wallet.loadErrorTitle")}
      description={t("wallet.loadErrorDescription")}
      action={
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-11 sm:h-8"
          onClick={() => void onRetry()}
        >
          <RefreshCw data-icon="inline-start" aria-hidden="true" />
          {t("wallet.retry")}
        </Button>
      }
    />
  );
}
