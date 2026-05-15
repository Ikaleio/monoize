import * as React from "react";

import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

function PageHeaderSkeleton({ className, ...props }: React.HTMLAttributes<HTMLDivElement>) {
  return (
    <div className={cn("flex flex-wrap items-center justify-between gap-4", className)} {...props}>
      <div className="min-w-0 space-y-2">
        <Skeleton className="h-8 w-48" />
        <Skeleton className="h-4 w-72 max-w-full" />
      </div>
      <div className="flex shrink-0 flex-wrap items-center gap-2">
        <Skeleton className="h-10 w-24" />
        <Skeleton className="h-10 w-24" />
      </div>
    </div>
  );
}

interface TablePageSkeletonProps extends React.HTMLAttributes<HTMLDivElement> {
  rows?: number;
  columns?: number;
  showToolbar?: boolean;
}

function TablePageSkeleton({
  className,
  rows = 6,
  columns = 4,
  showToolbar = true,
  ...props
}: TablePageSkeletonProps) {
  return (
    <div className={cn("space-y-6", className)} {...props}>
      <PageHeaderSkeleton />
      {showToolbar ? (
        <div className="flex flex-wrap items-center justify-between gap-3">
          <Skeleton className="h-10 w-full sm:w-64" />
          <Skeleton className="h-10 w-28" />
        </div>
      ) : null}
      <Card>
        <CardContent className="space-y-3 p-4">
          {Array.from({ length: rows }).map((_, rowIndex) => (
            <div key={rowIndex} className="grid gap-3" style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}>
              {Array.from({ length: columns }).map((__, columnIndex) => (
                <Skeleton key={columnIndex} className="h-6" />
              ))}
            </div>
          ))}
        </CardContent>
      </Card>
    </div>
  );
}

interface CardsPageSkeletonProps extends React.HTMLAttributes<HTMLDivElement> {
  count?: number;
  gridClassName?: string;
}

function CardsPageSkeleton({
  className,
  count = 6,
  gridClassName,
  ...props
}: CardsPageSkeletonProps) {
  return (
    <div className={cn("space-y-6", className)} {...props}>
      <PageHeaderSkeleton />
      <div className={cn("grid gap-4 sm:grid-cols-2 lg:grid-cols-3", gridClassName)}>
        {Array.from({ length: count }).map((_, index) => (
          <Card key={index}>
            <CardContent className="space-y-4 p-6">
              <Skeleton className="h-5 w-2/3" />
              <Skeleton className="h-4 w-full" />
              <Skeleton className="h-4 w-5/6" />
            </CardContent>
          </Card>
        ))}
      </div>
    </div>
  );
}

export { PageHeaderSkeleton, TablePageSkeleton, CardsPageSkeleton };
