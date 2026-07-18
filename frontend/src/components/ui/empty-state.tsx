import * as React from "react";

import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export interface EmptyStateProps extends Omit<React.HTMLAttributes<HTMLDivElement>, "title"> {
  variant?: "card" | "inline";
  icon?: React.ReactNode;
  title: React.ReactNode;
  description?: React.ReactNode;
  action?: React.ReactNode;
}

const EmptyState = React.forwardRef<HTMLDivElement, EmptyStateProps>(
  ({ className, variant = "card", icon, title, description, action, ...props }, ref) => {
    const content = (
      <div
        ref={ref}
        className={cn("flex flex-col items-center justify-center px-6 py-10 text-center", className)}
        {...props}
      >
        {icon ? <div className="mb-4 text-muted-foreground">{icon}</div> : null}
        <h3 className="text-base font-semibold text-foreground">{title}</h3>
        {description ? <p className="mt-2 max-w-md text-sm text-muted-foreground">{description}</p> : null}
        {action ? <div className="mt-6">{action}</div> : null}
      </div>
    );

    if (variant === "inline") {
      return content;
    }

    return <Card>{content}</Card>;
  }
);
EmptyState.displayName = "EmptyState";

export { EmptyState };
