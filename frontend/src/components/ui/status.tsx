import * as React from "react";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { cn } from "@/lib/utils";

type StatusVariant = "success" | "warning" | "info" | "destructive";

const statusClasses: Record<StatusVariant, { badge: string; dot: string; alert: string }> = {
  success: {
    badge: "border-success-border bg-success-soft text-success-foreground",
    dot: "bg-success",
    alert: "border-success-border bg-success-soft text-success-foreground [&>svg]:text-success",
  },
  warning: {
    badge: "border-warning-border bg-warning-soft text-warning-foreground",
    dot: "bg-warning",
    alert: "border-warning-border bg-warning-soft text-warning-foreground [&>svg]:text-warning",
  },
  info: {
    badge: "border-info-border bg-info-soft text-info-foreground",
    dot: "bg-info",
    alert: "border-info-border bg-info-soft text-info-foreground [&>svg]:text-info",
  },
  destructive: {
    badge: "border-destructive/30 bg-destructive/10 text-destructive",
    dot: "bg-destructive",
    alert: "border-destructive/30 bg-destructive/10 text-destructive [&>svg]:text-destructive",
  },
};

export interface StatusBadgeProps extends React.HTMLAttributes<HTMLSpanElement> {
  variant: StatusVariant;
}

function StatusBadge({ className, variant, ...props }: StatusBadgeProps) {
  return (
    <span
      className={cn(
        "inline-flex min-w-0 max-w-full shrink-0 flex-nowrap items-center truncate rounded-full border px-2.5 py-0.5 text-xs font-medium",
        statusClasses[variant].badge,
        className
      )}
      {...props}
    />
  );
}

export interface StatusDotProps extends React.HTMLAttributes<HTMLSpanElement> {
  variant: StatusVariant;
}

function StatusDot({ className, variant, ...props }: StatusDotProps) {
  return <span className={cn("inline-block h-2 w-2 rounded-full", statusClasses[variant].dot, className)} {...props} />;
}

export interface StatusAlertProps extends React.HTMLAttributes<HTMLDivElement> {
  variant: StatusVariant;
  title?: React.ReactNode;
  description?: React.ReactNode;
  icon?: React.ReactNode;
}

function StatusAlert({ className, variant, title, description, icon, children, ...props }: StatusAlertProps) {
  return (
    <Alert className={cn(statusClasses[variant].alert, className)} {...props}>
      {icon}
      {title ? <AlertTitle>{title}</AlertTitle> : null}
      {description ? <AlertDescription>{description}</AlertDescription> : null}
      {children}
    </Alert>
  );
}

export { StatusBadge, StatusDot, StatusAlert, type StatusVariant };
