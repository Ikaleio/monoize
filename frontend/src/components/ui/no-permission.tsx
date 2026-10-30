import { Link } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { ShieldAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";
import { PageWrapper } from "@/components/ui/motion";

/** Shared full-page state for a route the signed-in user is not allowed to open. */
export function NoPermissionState({ title }: { title?: string }) {
  const { t } = useTranslation();
  return (
    <PageWrapper className="space-y-6">
      {title ? <PageHeader title={title} /> : null}
      <EmptyState
        icon={<ShieldAlert aria-hidden="true" className="size-8" />}
        title={t("guards.forbiddenTitle")}
        description={t("guards.forbiddenDescription")}
        action={
          <Button asChild>
            <Link to="/dashboard">{t("guards.backToDashboard")}</Link>
          </Button>
        }
      />
    </PageWrapper>
  );
}
