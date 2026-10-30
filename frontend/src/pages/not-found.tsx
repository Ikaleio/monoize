import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { PageHeader } from "@/components/ui/page-header";
import { PageWrapper } from "@/components/ui/motion";

/** Unknown route inside the dashboard shell (dashboard-ui-layout.spec.md DL6b). */
export function NotFoundPage() {
  const { t } = useTranslation();
  return (
    <PageWrapper className="space-y-6">
      <PageHeader
        title={t("errors.notFoundTitle")}
        description={t("errors.notFoundDescription")}
      />
      <Button asChild>
        <Link to="/dashboard">{t("guards.backToDashboard")}</Link>
      </Button>
    </PageWrapper>
  );
}
