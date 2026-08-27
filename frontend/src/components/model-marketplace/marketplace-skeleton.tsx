import { getModelCardSpan } from "@/components/model-marketplace/model-grid";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";

export function ModelMarketplaceSkeleton() {
  return (
    <div className="flex flex-col gap-6" aria-hidden="true">
      <div className="flex flex-col gap-3">
        <Skeleton className="h-8 w-56" />
        <Skeleton className="h-5 w-80 max-w-full" />
      </div>
      <Skeleton className="h-9 w-full max-w-xl" />
      <div className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-12">
        {Array.from({ length: 7 }, (_, index) => (
          <div key={index} className={getModelCardSpan(index)}>
            <Card className="flex h-full flex-col overflow-hidden">
              <CardHeader>
                <div className="flex items-center justify-between gap-4">
                  <Skeleton className="size-10" />
                  <Skeleton className="h-7 w-20" />
                </div>
                <div className="flex flex-col gap-2">
                  <Skeleton className="h-5 w-2/3" />
                  <Skeleton className="h-5 w-1/3" />
                </div>
              </CardHeader>
              <CardContent className="mt-auto grid grid-cols-2 gap-4">
                <Skeleton className="h-12 w-full" />
                <Skeleton className="h-12 w-full" />
              </CardContent>
              <Separator />
              <CardFooter className="grid grid-cols-2 gap-4 p-6 pt-4">
                <Skeleton className="h-10 w-full" />
                <Skeleton className="h-10 w-full" />
              </CardFooter>
            </Card>
          </div>
        ))}
      </div>
    </div>
  );
}
