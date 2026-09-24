import { t } from "@fvoci/i18n";
import { useId } from "react";
import { TaskForm } from "./task-form";
import type { CreateTaskBody } from "./queries";
import "@/features/projects/projects.css";

export function TaskCreateDialog({
  open,
  projectKey,
  pending,
  error,
  onOpenChange,
  onSubmit,
}: {
  open: boolean;
  projectKey: string;
  pending?: boolean;
  error?: string | null;
  onOpenChange: (open: boolean) => void;
  onSubmit: (values: Pick<CreateTaskBody, "title" | "type">) => void | Promise<void>;
}) {
  const titleId = useId();
  if (!open) return null;
  return (
    <div className="project-dialog-backdrop">
      <div className="project-dialog" role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <nav aria-label={t("nav.breadcrumb")}>
          <ol className="task-home__crumb" style={{ display: "flex", gap: "0.4rem", listStyle: "none", padding: 0 }}>
            <li className="tabular-nums">{projectKey}</li>
            <li aria-hidden="true">/</li>
            <li>{t("task.create.new")}</li>
          </ol>
        </nav>
        <h2 id={titleId} className="project-dialog__title">
          {t("task.create.new")}
        </h2>
        <p className="task-home__note">{t("task.create.hint")}</p>
        <TaskForm
          pending={pending}
          onCancel={() => onOpenChange(false)}
          onSubmit={onSubmit}
        />
        {error ? (
          <p role="alert" className="task-form__alert">
            {error}
          </p>
        ) : null}
      </div>
    </div>
  );
}
