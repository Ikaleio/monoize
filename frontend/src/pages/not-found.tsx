import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { PageWrapper } from "@/components/ui/motion";

/** Unknown route inside the dashboard shell (dashboard-ui-layout.spec.md DL6b). */
export function NotFoundPage() {
  const { t } = useTranslation();
  return (
    <PageWrapper>
      {/* Not EmptyState: this replaces the whole page, so its title must be the page h1. */}
      <Card className="flex flex-col items-center justify-center px-6 py-10 text-center">
        <h1 className="text-base font-semibold text-foreground">{t("errors.notFoundTitle")}</h1>
        <p className="mt-2 max-w-md text-sm text-muted-foreground">
          {t("errors.notFoundDescription")}
        </p>
        <div className="mt-6">
          <Button asChild>
            <Link to="/dashboard">{t("guards.backToDashboard")}</Link>
          </Button>
        </div>
      </Card>
    </PageWrapper>
  );
}
