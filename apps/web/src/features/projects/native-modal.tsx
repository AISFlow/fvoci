import { useEffect, useRef, type ReactNode } from "react";

export function NativeModal({
  open,
  labelledBy,
  onClose,
  children,
}: {
  open: boolean;
  labelledBy: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const openerRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!open) {
      openerRef.current?.focus();
      openerRef.current = null;
      return;
    }
    const dialog = dialogRef.current;
    if (!openerRef.current && document.activeElement instanceof HTMLElement) {
      openerRef.current = document.activeElement;
    }
    if (dialog && !dialog.open) dialog.showModal();
  }, [open]);

  if (!open) return null;

  return (
    <dialog
      ref={dialogRef}
      className="project-dialog"
      aria-labelledby={labelledBy}
      aria-modal="true"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
    >
      {children}
    </dialog>
  );
}
