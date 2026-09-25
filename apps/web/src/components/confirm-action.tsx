// Adapted from source apps/web/src/components/confirm-action.tsx using this
// app's alertdialog pattern (see features/settings/workspace-tokens.tsx).
import { t } from "@fvoci/i18n";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

export function ConfirmActionButton({
  title,
  description,
  actionLabel,
  disabled,
  destructive = true,
  triggerVariant = "outline",
  onConfirm,
  children,
}: {
  title: string;
  description: string;
  actionLabel: string;
  disabled?: boolean;
  destructive?: boolean;
  triggerVariant?: "default" | "outline" | "destructive";
  onConfirm: () => Promise<void> | void;
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const titleId = useId();
  const confirmRef = useRef<HTMLButtonElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (open) confirmRef.current?.focus();
  }, [open]);

  function close() {
    setOpen(false);
    triggerRef.current?.focus();
  }

  return (
    <>
      <Button
        ref={triggerRef}
        type="button"
        variant={triggerVariant}
        size="sm"
        disabled={disabled}
        onClick={() => setOpen(true)}
      >
        {children}
      </Button>
      {open ? (
        <div
          role="alertdialog"
          aria-modal="true"
          aria-labelledby={titleId}
          className="fixed inset-0 z-20 flex items-center justify-center bg-black/40 p-4"
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              close();
            }
          }}
        >
          <div className="max-w-md rounded-md border border-border bg-background p-4">
            <h2 id={titleId} className="text-title">
              {title}
            </h2>
            <p className="mt-2 break-keep text-ui text-muted-foreground">{description}</p>
            <div className="mt-4 flex justify-end gap-2">
              <Button type="button" variant="outline" size="sm" onClick={close}>
                {t("common.cancel")}
              </Button>
              <Button
                ref={confirmRef}
                type="button"
                size="sm"
                variant={destructive ? "destructive" : "default"}
                disabled={busy}
                onClick={() => {
                  setBusy(true);
                  void Promise.resolve(onConfirm()).finally(() => {
                    setBusy(false);
                    close();
                  });
                }}
              >
                {actionLabel}
              </Button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
