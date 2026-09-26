import * as React from "react";
import { Slot } from "@radix-ui/react-slot";

import { cn } from "@/lib/utils";

/*
 * Responsive list (frontend-design-system.spec.md §6.1). Layout follows the
 * list's own width through container queries, so one markup serves the stacked
 * narrow layout and the aligned wide layout. Wide mode starts at 56rem. The
 * middle range is bounded on both sides so that it never depends on the order
 * Tailwind emits the two arbitrary container variants.
 */

type DataListAlign = "start" | "end";

interface DataListProps extends React.HTMLAttributes<HTMLDivElement> {
  /** Wide-mode `grid-template-columns` value shared by the header and every row. */
  columns: string;
}

const DataList = React.forwardRef<HTMLDivElement, DataListProps>(
  ({ className, columns, style, ...props }, ref) => (
    <div
      ref={ref}
      className={cn("min-w-0 [container-type:inline-size]", className)}
      style={{ ...style, "--data-list-columns": columns } as React.CSSProperties}
      {...props}
    />
  ),
);
DataList.displayName = "DataList";

const DataListHeader = React.forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      aria-hidden="true"
      className={cn(
        "sticky top-0 z-10 hidden h-9 items-center gap-x-4 border-b bg-card px-4 text-xs font-medium text-muted-foreground",
        "[@container(min-width:56rem)]:grid [@container(min-width:56rem)]:grid-cols-[var(--data-list-columns)]",
        className,
      )}
      {...props}
    />
  ),
);
DataListHeader.displayName = "DataListHeader";

interface DataListHeadProps extends React.HTMLAttributes<HTMLDivElement> {
  align?: DataListAlign;
}

const DataListHead = React.forwardRef<HTMLDivElement, DataListHeadProps>(
  ({ className, align = "start", ...props }, ref) => (
    <div
      ref={ref}
      className={cn("min-w-0 truncate", align === "end" ? "text-end" : "text-start", className)}
      {...props}
    />
  ),
);
DataListHead.displayName = "DataListHead";

const DataListBody = React.forwardRef<HTMLUListElement, React.HTMLAttributes<HTMLUListElement>>(
  ({ className, ...props }, ref) => (
    <ul ref={ref} className={cn("divide-y", className)} {...props} />
  ),
);
DataListBody.displayName = "DataListBody";

interface DataListRowProps extends React.LiHTMLAttributes<HTMLLIElement> {
  asChild?: boolean;
}

const DataListRow = React.forwardRef<HTMLLIElement, DataListRowProps>(
  ({ className, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "li";
    return (
      <Comp
        ref={ref}
        className={cn(
          "grid items-center gap-x-6 gap-y-2 px-4 py-3 text-sm transition-colors hover:bg-muted/50",
          "[@container(min-width:36rem)_and_(max-width:55.99rem)]:grid-cols-2",
          "[@container(min-width:56rem)]:grid-cols-[var(--data-list-columns)] [@container(min-width:56rem)]:gap-x-4",
          className,
        )}
        {...props}
      />
    );
  },
);
DataListRow.displayName = "DataListRow";

const PRIMARY_CELL = "order-first col-span-full [@container(min-width:56rem)]:order-none [@container(min-width:56rem)]:col-auto";

interface DataListCellProps extends React.HTMLAttributes<HTMLDivElement> {
  /** Visible before the value below wide mode; visually hidden in wide mode. */
  label?: React.ReactNode;
  align?: DataListAlign;
  /** Spans the full row and renders first below wide mode. Use for the identity cell. */
  primary?: boolean;
}

const DataListCell = React.forwardRef<HTMLDivElement, DataListCellProps>(
  ({ className, label, align = "start", primary = false, children, ...props }, ref) => {
    const wideAlign = align === "end" ? "[@container(min-width:56rem)]:text-end" : "[@container(min-width:56rem)]:text-start";
    if (label == null) {
      return (
        <div
          ref={ref}
          className={cn("min-w-0", primary && PRIMARY_CELL, wideAlign, className)}
          {...props}
        >
          {children}
        </div>
      );
    }
    return (
      <div
        ref={ref}
        className={cn(
          "flex min-w-0 items-center justify-between gap-3 [@container(min-width:56rem)]:block",
          primary && PRIMARY_CELL,
          className,
        )}
        {...props}
      >
        <span className="shrink-0 text-muted-foreground [@container(min-width:56rem)]:sr-only">{label}</span>
        <div className={cn("min-w-0 text-end", wideAlign)}>{children}</div>
      </div>
    );
  },
);
DataListCell.displayName = "DataListCell";

const DataListActions = React.forwardRef<HTMLDivElement, React.HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      className={cn(
        "order-last col-span-full flex min-w-0 items-center justify-end gap-1 [@container(min-width:56rem)]:order-none [@container(min-width:56rem)]:col-auto",
        className,
      )}
      {...props}
    />
  ),
);
DataListActions.displayName = "DataListActions";

export {
  DataList,
  DataListHeader,
  DataListHead,
  DataListBody,
  DataListRow,
  DataListCell,
  DataListActions,
  type DataListAlign,
};
