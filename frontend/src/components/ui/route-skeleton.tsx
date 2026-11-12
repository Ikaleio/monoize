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

export type RouteSkeletonKind =
  | "table"
  | "cards"
  | "providers"
  | "dashboard"
  | "admin"
  | "settings"
  | "account"
  | "wallet"
  | "playground"
  | "not-found"
  | "login";

function PanelSkeleton({
  className,
  rows = 3,
}: {
  className?: string;
  rows?: number;
}) {
  return (
    <Card className={className}>
      <CardHeader className="space-y-2">
        <Skeleton className="h-5 w-40 max-w-full" />
        <Skeleton className="h-4 w-64 max-w-full" />
      </CardHeader>
      <CardContent className="space-y-3">
        {Array.from({ length: rows }, (_, index) => (
          <Skeleton key={index} className="h-8 w-full" />
        ))}
      </CardContent>
    </Card>
  );
}

function DashboardSkeleton() {
  return (
    <div className="space-y-4">
      <PageHeaderSkeleton />
      <Card className="grid gap-4 p-6 sm:grid-cols-2 lg:grid-cols-[1.35fr_repeat(4,minmax(0,1fr))]">
        {Array.from({ length: 5 }, (_, index) => (
          <div key={index} className="space-y-3">
            <Skeleton className="h-4 w-24" />
            <Skeleton className="h-8 w-32 max-w-full" />
          </div>
        ))}
      </Card>
      <Card>
        <CardHeader>
          <Skeleton className="h-6 w-40" />
        </CardHeader>
        <CardContent>
          <Skeleton className="h-72 w-full" />
        </CardContent>
      </Card>
      <div className="grid gap-4 lg:grid-cols-3">
        <PanelSkeleton className="lg:col-span-2" />
        <PanelSkeleton />
      </div>
      <PanelSkeleton />
    </div>
  );
}

function LoginSkeleton() {
  return (
    <div className="h-dvh overflow-y-auto overscroll-contain p-4">
      <div className="flex min-h-full flex-col gap-4">
        <div className="flex justify-end gap-2">
          <Skeleton className="size-9" />
          <Skeleton className="size-9" />
        </div>
        <Card className="my-auto w-full max-w-md shrink-0 self-center overflow-hidden">
          <CardHeader className="items-center space-y-2 text-center">
            <Skeleton className="mb-4 size-14 rounded-2xl" />
            <Skeleton className="h-8 w-48 max-w-full" />
            <Skeleton className="h-4 w-40 max-w-full" />
            <Skeleton className="h-4 w-48 max-w-full" />
          </CardHeader>
          <CardContent className="space-y-4">
            {[0, 1].map((index) => (
              <div key={index} className="space-y-2">
                <Skeleton className="h-4 w-20" />
                <Skeleton className="h-9 w-full" />
              </div>
            ))}
            <Skeleton className="h-9 w-full" />
            <Skeleton className="mx-auto h-5 w-56 max-w-full" />
          </CardContent>
        </Card>
      </div>
    </div>
  );
}

function DashboardRouteSkeleton({
  kind,
}: {
  kind: Exclude<RouteSkeletonKind, "login">;
}) {
  switch (kind) {
    case "dashboard":
      return <DashboardSkeleton />;
    case "admin":
      return (
        <div className="space-y-6">
          <PageHeaderSkeleton />
          <PanelSkeleton rows={5} />
          <div className="grid gap-6 lg:grid-cols-12">
            <PanelSkeleton className="lg:col-span-7" rows={4} />
            <PanelSkeleton className="lg:col-span-5" rows={6} />
          </div>
        </div>
      );
    case "cards":
      return (
        <CardsPageSkeleton
          count={3}
          gridClassName="grid-cols-1 sm:grid-cols-1 md:grid-cols-2 lg:grid-cols-2 xl:grid-cols-3"
        />
      );
    case "providers":
      return (
        <CardsPageSkeleton
          count={3}
          gridClassName="grid-cols-1 sm:grid-cols-1 lg:grid-cols-1"
        />
      );
    case "settings":
      return <SettingsPageSkeleton />;
    case "account":
      return (
        <div className="space-y-6">
          <PageHeaderSkeleton />
          <PanelSkeleton rows={4} />
          <PanelSkeleton rows={3} />
          <PanelSkeleton rows={2} />
        </div>
      );
    case "wallet":
      return (
        <div className="space-y-6">
          <PageHeaderSkeleton />
          <PanelSkeleton rows={1} />
          <Skeleton className="h-10 w-72 max-w-full" />
          <PanelSkeleton rows={4} />
        </div>
      );
    case "playground":
      return (
        <div className="flex min-h-[70dvh] flex-col gap-4">
          <div className="flex flex-wrap justify-between gap-3">
            <Skeleton className="h-9 w-40" />
            <Skeleton className="h-9 w-20" />
          </div>
          <div className="flex flex-1 items-center justify-center">
            <Skeleton className="h-16 w-72 max-w-full" />
          </div>
          <Card className="space-y-4 p-4">
            <Skeleton className="h-20 w-full" />
            <div className="flex flex-wrap gap-2">
              <Skeleton className="h-8 w-36" />
              <Skeleton className="h-8 w-24" />
            </div>
          </Card>
        </div>
      );
    case "not-found":
      return (
        <Card className="flex flex-col items-center gap-4 px-6 py-10">
          <Skeleton className="h-6 w-40" />
          <Skeleton className="h-4 w-72 max-w-full" />
          <Skeleton className="h-10 w-32" />
        </Card>
      );
    default:
      return <TablePageSkeleton />;
  }
}

export function RouteSkeleton({ kind }: { kind: RouteSkeletonKind }) {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      aria-busy="true"
      aria-label={t("common.loading")}
      data-route-loading={kind}
    >
      <span className="sr-only">{t("common.loading")}</span>
      <div aria-hidden="true">
        {kind === "login" ? (
          <LoginSkeleton />
        ) : (
          <PageWrapper instant className="min-w-0 pb-6">
            <DashboardRouteSkeleton kind={kind} />
          </PageWrapper>
        )}
      </div>
    </div>
  );
}
