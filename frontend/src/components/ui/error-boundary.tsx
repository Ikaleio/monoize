import * as React from "react";
import { Link, useRouteError } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { PageWrapper } from "@/components/ui/motion";

/**
 * Route `errorElement` for the dashboard shell. It replaces only the page content, so
 * the sidebar stays usable, and React Router clears it on the next navigation.
 */
export function RouteErrorBoundary() {
  const { t } = useTranslation();
  const error = useRouteError();

  React.useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <PageWrapper>
      {/* Not EmptyState: this replaces the whole page, so its title must be the page h1. */}
      <Card
        role="alert"
        className="flex flex-col items-center justify-center px-6 py-10 text-center"
      >
        <h1 className="text-base font-semibold text-foreground">{t("errors.pageTitle")}</h1>
        <p className="mt-2 max-w-md text-sm text-muted-foreground">
          {t("errors.pageDescription")}
        </p>
        <div className="mt-6 flex flex-wrap justify-center gap-2">
          {/* A full reload also recovers from stale chunks after a deployment. */}
          <Button type="button" onClick={() => window.location.reload()}>
            {t("errors.reload")}
          </Button>
          <Button asChild variant="outline">
            <Link to="/dashboard">{t("guards.backToDashboard")}</Link>
          </Button>
        </div>
      </Card>
    </PageWrapper>
  );
}

interface PanelErrorBoundaryProps {
  children: React.ReactNode;
  /** Names the failed panel in the fallback. */
  title?: string;
}

interface PanelErrorBoundaryState {
  error: unknown;
}

function PanelErrorFallback({ title, onRetry }: { title?: string; onRetry: () => void }) {
  const { t } = useTranslation();
  return (
    <Alert variant="destructive">
      {title ? <AlertTitle>{title}</AlertTitle> : null}
      <AlertDescription className="flex flex-wrap items-center justify-between gap-3">
        <span className="min-w-0 flex-1 basis-40">{t("errors.panelDescription")}</span>
        <Button type="button" variant="outline" size="sm" onClick={onRetry}>
          {t("common.retry")}
        </Button>
      </AlertDescription>
    </Alert>
  );
}

/** Contains a render failure inside one independent panel so the rest of the page stays usable. */
export class PanelErrorBoundary extends React.Component<
  PanelErrorBoundaryProps,
  PanelErrorBoundaryState
> {
  state: PanelErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): PanelErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: unknown, info: React.ErrorInfo) {
    console.error(error, info.componentStack);
  }

  render() {
    if (this.state.error !== null) {
      return (
        <PanelErrorFallback
          title={this.props.title}
          onRetry={() => this.setState({ error: null })}
        />
      );
    }
    return this.props.children;
  }
}
