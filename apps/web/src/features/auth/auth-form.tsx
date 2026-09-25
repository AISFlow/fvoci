// Adapted from fvoci/FVOCI apps/web/src/features/auth/auth-form.tsx
import type { ReactElement, ReactNode } from "react";
import { cloneElement, isValidElement } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/cn";

export const authPrimaryButtonClass = "h-11 w-full text-ui font-medium";
export const authOutlineButtonClass = "h-11 w-full text-ui";
// Anchor styled like an outline button: OIDC starts are plain browser
// navigations, not fetches.
export const authOutlineLinkClass =
  "inline-flex h-11 w-full items-center justify-center rounded-md border border-border bg-background px-6 text-ui font-medium transition-colors hover:bg-accent hover:text-foreground";

export function AuthField({
  id,
  label,
  error,
  hint,
  children,
}: {
  id: string;
  label: string;
  error?: string;
  hint?: string;
  children: ReactNode;
}) {
  const errorId = error ? `${id}-error` : undefined;
  const hintId = hint ? `${id}-hint` : undefined;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ") || undefined;
  const control = isValidElement(children)
    ? cloneElement(children as ReactElement<Record<string, unknown>>, {
        id: (children.props as { id?: string }).id ?? id,
        "aria-describedby": describedBy,
        "aria-invalid": error ? true : undefined,
      })
    : children;

  return (
    <div className="auth-shell__field">
      <Label htmlFor={id} className="auth-shell__label">{label}</Label>
      {control}
      {hint ? <p id={hintId} className="auth-shell__hint">{hint}</p> : null}
      {error ? (
        <p id={errorId} role="alert" className="auth-shell__alert">{error}</p>
      ) : null}
    </div>
  );
}

export function AuthInput({
  className,
  ...props
}: React.ComponentProps<typeof Input>) {
  return <Input className={cn("h-11", className)} {...props} />;
}

export function AuthAlert({ children }: { children: ReactNode }) {
  return <p role="alert" className="auth-shell__alert">{children}</p>;
}

export function AuthStatus({ children }: { children: ReactNode }) {
  return <p role="status" className="auth-shell__status break-keep">{children}</p>;
}

export function AuthDisclosure({
  trigger,
  open,
  onOpenChange,
  children,
}: {
  trigger: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  children: ReactNode;
}) {
  if (!open) {
    return (
      <Button type="button" variant="link" onClick={() => onOpenChange(true)}>
        {trigger}
      </Button>
    );
  }
  return <div className="auth-shell__stack">{children}</div>;
}
