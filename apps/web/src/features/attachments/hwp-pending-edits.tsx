import { t } from "@fvoci/i18n";
import { useEffect, useId, useRef, type ReactNode } from "react";
import { useBlocker } from "react-router-dom";
import { Button } from "@/components/ui/button";

/**
 * "Leave and lose the edits?" (source `ConfirmActionDialog` with the
 * `task.body.unsaved.*` copy), in this app's alertdialog pattern
 * (`components/confirm-action.tsx`). Escape and 취소 keep the edits;
 * `actionLabel` names what discarding them does (default 나가기).
 */
export function DiscardEditsDialog({
  actionLabel = t("task.body.unsaved.leave"),
  onConfirm,
  onCancel,
}: {
  actionLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
}): ReactNode {
  const titleId = useId();
  const cancelRef = useRef<HTMLButtonElement>(null);
  // Focus the safe choice: Enter keeps the edits.
  useEffect(() => {
    cancelRef.current?.focus();
  }, []);
  return (
    <div
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={titleId}
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
          {t("task.body.unsaved.title")}
        </h2>
        <p className="mt-2 break-keep text-ui text-muted-foreground">{t("task.body.unsaved.body")}</p>
        <div className="mt-4 flex justify-end gap-2">
          <Button ref={cancelRef} type="button" variant="outline" size="sm" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button type="button" size="sm" variant="destructive" onClick={onConfirm}>
            {actionLabel}
          </Button>
        </div>
      </div>
    </div>
  );
}

/**
 * Source `PendingEditsGuard`, mounted only while there are unsaved edits: an
 * in-app navigation (link, `navigate`, back/forward) waits for the dialog,
 * and closing or reloading the tab gets the browser's own prompt. The
 * blocker needs the app's data router.
 */
export function PendingEditsGuard(): ReactNode {
  const blocker = useBlocker(true);
  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      // Chromium before 119 and Safari only prompt when returnValue is set.
      event.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, []);
  if (blocker.state !== "blocked") return null;
  return <DiscardEditsDialog onConfirm={() => blocker.proceed()} onCancel={() => blocker.reset()} />;
}
