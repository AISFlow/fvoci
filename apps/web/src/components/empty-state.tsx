import type { ReactNode } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";

interface EmptyStateProps {
  icon?: ReactNode;
  title: string;
  description?: string;
  actionTo?: string;
  actionLabel?: string;
  onAction?: () => void;
  actionDisabled?: boolean;
}

export function EmptyState({
  icon,
  title,
  description,
  actionTo,
  actionLabel,
  onAction,
  actionDisabled = false,
}: EmptyStateProps) {
  return (
    <div className="mx-auto flex w-full max-w-lg flex-1 flex-col items-start justify-center gap-3 px-6 py-12 sm:px-8">
      {icon ? (
        <span
          className="mb-2 flex size-10 items-center justify-center rounded-lg bg-muted text-title text-muted-foreground"
          aria-hidden
        >
          {icon}
        </span>
      ) : null}
      <p className="break-keep text-title font-semibold text-foreground">{title}</p>
      {description ? (
        <p className="max-w-prose break-keep text-ui leading-relaxed text-muted-foreground">
          {description}
        </p>
      ) : null}
      {actionLabel && onAction ? (
        <Button
          type="button"
          className="mt-2"
          size="sm"
          disabled={actionDisabled}
          onClick={onAction}
        >
          {actionLabel}
        </Button>
      ) : actionTo && actionLabel ? (
        <Link to={actionTo} className="mt-2 inline-flex h-8 items-center rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground hover:opacity-90">
          {actionLabel}
        </Link>
      ) : null}
    </div>
  );
}
