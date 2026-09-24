import { t } from "@fvoci/i18n";
import { useId } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { taskCreatePayload } from "./create-payload";
import type { CreateTaskBody } from "./queries";
import { TASK_TYPES, TASK_TYPE_LABELS, isTaskType, type TaskType } from "./task-types";
import "@/features/projects/projects.css";

interface TaskFormValues {
  title: string;
  type: TaskType;
}

export function TaskForm({
  pending,
  onCancel,
  onSubmit,
}: {
  pending?: boolean;
  onCancel: () => void;
  onSubmit: (values: Pick<CreateTaskBody, "title" | "type">) => void | Promise<void>;
}) {
  const titleId = useId();
  const typeId = useId();
  const form = useForm<TaskFormValues>({
    defaultValues: { title: "", type: "task" },
  });
  const type = form.watch("type");
  const titleError = form.formState.errors.title?.message;
  const parentError = form.formState.errors.type?.message;

  return (
    <form
      className="task-form"
      noValidate
      onSubmit={form.handleSubmit(async (values) => {
        const parsed = taskCreatePayload(values);
        if (!parsed.ok) {
          if (parsed.issue === "parent") {
            form.setError("type", { message: t("task.parent.required") });
          } else if (parsed.issue === "type") {
            form.setError("type", { message: t("task.form.type.label") });
          } else {
            form.setError("title", { message: t("task.form.titleRequired") });
          }
          return;
        }
        await onSubmit(parsed.body);
      })}
    >
      <div className="task-form__field">
        <Label htmlFor={titleId}>{t("task.col.title")}</Label>
        <Input
          id={titleId}
          placeholder={t("task.form.placeholder")}
          aria-invalid={titleError ? true : undefined}
          autoFocus
          disabled={pending}
          {...form.register("title")}
        />
        {titleError ? (
          <p className="task-form__alert" role="alert">
            {titleError}
          </p>
        ) : null}
      </div>
      <div className="task-form__field">
        <Label htmlFor={typeId}>{t("task.form.type.label")}</Label>
        <select
          id={typeId}
          disabled={pending}
          value={type}
          onChange={(event) => {
            const next = event.target.value;
            if (isTaskType(next)) form.setValue("type", next);
          }}
        >
          {TASK_TYPES.map((value) => (
            <option key={value} value={value}>
              {TASK_TYPE_LABELS[value]}
            </option>
          ))}
        </select>
        {parentError ? (
          <p className="task-form__alert" role="alert">
            {parentError}
          </p>
        ) : null}
      </div>
      <div className="task-form__actions">
        <Button type="button" variant="outline" onClick={onCancel}>
          {t("task.create.cancel")}
        </Button>
        <Button type="submit" disabled={pending}>
          {pending ? t("task.create.pending") : t("task.create")}
        </Button>
      </div>
    </form>
  );
}
