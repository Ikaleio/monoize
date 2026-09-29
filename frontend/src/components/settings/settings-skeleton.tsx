import { Card } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { SettingsBody } from "./settings-layout";

/**
 * Loading skeleton with the ready page geometry: header, search and category
 * navigation, then one category title above a card of setting rows (SSU-23).
 */
export function SettingsPageSkeleton() {
  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-2 pb-1">
        <Skeleton className="h-8 w-40" />
        <Skeleton className="h-4 w-64 max-w-full" />
      </div>
      <SettingsBody
        aside={
          <>
            <Skeleton className="h-9 w-full" />
            <div className="flex gap-1 overflow-hidden [@container(min-width:56rem)]:flex-col">
              {Array.from({ length: 9 }, (_, index) => (
                <Skeleton
                  key={index}
                  className="h-11 w-28 shrink-0 sm:h-9 [@container(min-width:56rem)]:w-full"
                />
              ))}
            </div>
          </>
        }
      >
        <div className="flex flex-col gap-5">
          <div className="flex flex-col gap-2">
            <Skeleton className="h-6 w-44" />
            <Skeleton className="h-4 w-full max-w-md" />
          </div>
          <Card className="divide-y [container-type:inline-size]">
            {Array.from({ length: 4 }, (_, index) => (
              <div
                key={index}
                className="grid gap-3 px-4 py-4 sm:px-6 [@container(min-width:40rem)]:grid-cols-[minmax(0,1fr)_20rem] [@container(min-width:40rem)]:gap-x-8"
              >
                <div className="flex flex-col gap-2">
                  <Skeleton className="h-4 w-32" />
                  <Skeleton className="h-4 w-3/4" />
                </div>
                <Skeleton className="h-9 w-full" />
              </div>
            ))}
          </Card>
        </div>
      </SettingsBody>
    </div>
  );
}
