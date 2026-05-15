import * as React from "react";
import { Search } from "lucide-react";

import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export interface DataTableShellProps extends React.HTMLAttributes<HTMLDivElement> {
  toolbar?: React.ReactNode;
  emptyState?: React.ReactNode;
  isEmpty?: boolean;
}

const DataTableShell = React.forwardRef<HTMLDivElement, DataTableShellProps>(
  ({ className, toolbar, emptyState, isEmpty = false, children, ...props }, ref) => (
    <div ref={ref} className={cn("space-y-4", className)} {...props}>
      {toolbar ? <div className="flex flex-wrap items-center justify-between gap-3">{toolbar}</div> : null}
      {isEmpty && emptyState ? emptyState : <Card className="overflow-hidden">{children}</Card>}
    </div>
  )
);
DataTableShell.displayName = "DataTableShell";

export interface TableToolbarSearchProps extends React.ComponentProps<typeof Input> {
  containerClassName?: string;
  icon?: React.ReactNode;
}

const TableToolbarSearch = React.forwardRef<HTMLInputElement, TableToolbarSearchProps>(
  ({ className, containerClassName, icon, ...props }, ref) => (
    <div className={cn("relative w-full sm:w-64", containerClassName)}>
      {icon === null ? null : (
        <span className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-muted-foreground">
          {icon ?? <Search className="h-4 w-4" />}
        </span>
      )}
      <Input ref={ref} className={cn("w-full", icon === null ? undefined : "pl-9", className)} {...props} />
    </div>
  )
);
TableToolbarSearch.displayName = "TableToolbarSearch";

export { DataTableShell, TableToolbarSearch };
