import { useTranslation } from "react-i18next";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { PageWrapper } from "@/components/ui/motion";
import {
  CardsPageSkeleton,
  PageHeaderSkeleton,
  TablePageSkeleton,
} from "@/components/ui/page-skeleton";
import { Skeleton } from "@/components/ui/skeleton";
import { SettingsPageSkeleton } from "@/components/settings/settings-skeleton";

// Each kind reuses the shared skeleton the page shows while its data loads, or
// only the header for pages that lay out their own panels on mount, so the
// fallback does not promise structure the page then replaces.
export type RouteSkeletonKind = "table" | "cards" | "settings" | "header" | "none" | "login";

function dashboardSkeleton(kind: Exclude<RouteSkeletonKind, "login">, headerActions: number) {
  switch (kind) {
    case "table":
      return <TablePageSkeleton headerActions={headerActions} />;
    case "cards":
      return <CardsPageSkeleton headerActions={headerActions} />;
    case "settings":
      return <SettingsPageSkeleton />;
    case "header":
      return <PageHeaderSkeleton actions={headerActions} />;
    case "none":
      return null;
  }
}

function LoginSkeleton() {
  return (
    <div className="h-dvh overflow-y-auto overscroll-contain p-4">
      <div className="flex min-h-full flex-col gap-4">
        <div className="flex justify-end gap-2">
          <Skeleton className="size-9" />
          <Skeleton className="size-9" />
        </div>
        <Card className="my-auto w-full max-w-md shrink-0 self-center">
          <CardHeader className="items-center space-y-2">
            <Skeleton className="mb-4 size-14 rounded-2xl" />
            <Skeleton className="h-8 w-48" />
            <Skeleton className="h-4 w-40" />
            <Skeleton className="h-4 w-48" />
          </CardHeader>
          <CardContent className="space-y-4">
            {[0, 1].map((field) => (
              <div key={field} className="space-y-2">
                <Skeleton className="h-4 w-20" />
                <Skeleton className="h-9 w-full" />
              </div>
            ))}
            <Skeleton className="h-9 w-full" />
            <Skeleton className="mx-auto h-5 w-56" />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

/** Suspense fallback shown while a route's page module downloads. */
export function RouteSkeleton({
  kind,
  headerActions = 0,
}: {
  kind: RouteSkeletonKind;
  headerActions?: number;
}) {
  const { t } = useTranslation();
  return (
    <div role="status" aria-busy="true">
      <span className="sr-only">{t("common.loading")}</span>
      <div aria-hidden="true">
        {kind === "login" ? (
          <LoginSkeleton />
        ) : (
          <PageWrapper instant>{dashboardSkeleton(kind, headerActions)}</PageWrapper>
        )}
      </div>
    </div>
  );
}
