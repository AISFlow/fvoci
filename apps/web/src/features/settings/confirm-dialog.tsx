import { t } from "@fvoci/i18n";
import { useEffect, useId, useRef } from "react";
import { Button } from "@/components/ui/button";

/** Same alertdialog shape as the token revoke confirm: focus lands on the action, Escape cancels. */
export function ConfirmDialog({
  title,
  body,
  actionLabel,
  pending,
  error,
  onCancel,
  onConfirm,
}: {
  title: string;
  body: string;
  actionLabel: string;
  pending: boolean;
  error: string | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const titleId = useId();
  const bodyId = useId();
  const confirmRef = useRef<HTMLButtonElement>(null);
  const openerRef = useRef<Element | null>(null);

  useEffect(() => {
    openerRef.current = document.activeElement;
    confirmRef.current?.focus();
    return () => {
      if (openerRef.current instanceof HTMLElement && openerRef.current.isConnected) {
        openerRef.current.focus();
      }
    };
  }, []);

  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={titleId}
      aria-describedby={bodyId}
      className="fixed inset-0 z-20 flex items-center justify-center bg-black/40 p-4"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          onCancel();
        }
      }}
    >
      <div className="max-w-md rounded-md border border-border bg-background p-4">
        <h2 id={titleId} className="text-title">
          {title}
        </h2>
        <p id={bodyId} className="mt-2 text-ui break-all text-muted-foreground">
          {body}
        </p>
        {error ? (
          <p role="alert" className="settings-notice settings-notice--danger mt-2">
            {error}
          </p>
        ) : null}
        <div className="mt-4 flex justify-end gap-2">
          <Button type="button" variant="outline" size="sm" onClick={onCancel}>
            {t("common.dismiss")}
          </Button>
          <Button ref={confirmRef} type="button" size="sm" disabled={pending} onClick={onConfirm}>
            {actionLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}
