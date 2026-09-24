import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ProblemError } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import { canonicalizeProjectKey } from "@/lib/href";
import {
  PROJECT_DESCRIPTION_MAX,
  PROJECT_ICON_MAX,
  PROJECT_NAME_MAX,
  projectCreatePayload,
} from "./create-payload";
import type { CreateProjectBody } from "./queries";

const createSchema = z.object({
  key: z.string().min(1, "i18n:form.too_small"),
  name: z.string().trim().min(1, "i18n:form.too_small").max(PROJECT_NAME_MAX, "i18n:form.too_big"),
  visibility: z.enum(["workspace", "private"]),
  description: z.string().max(PROJECT_DESCRIPTION_MAX, "i18n:form.too_big").optional(),
  icon: z.string().max(PROJECT_ICON_MAX, "i18n:form.too_big").optional(),
});

type CreateForm = z.infer<typeof createSchema>;

export function CreateProjectForm({
  pending,
  onSubmit,
  onCancel,
}: {
  pending?: boolean;
  onSubmit: (input: CreateProjectBody) => Promise<void>;
  onCancel: () => void;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm<CreateForm>({
    resolver: zodResolver(createSchema),
    defaultValues: {
      key: "",
      name: "",
      visibility: "workspace",
      description: "",
      icon: "",
    },
  });
  const fieldError =
    formFieldMessage(form.formState.errors.key, "key") ??
    formFieldMessage(form.formState.errors.name, "name") ??
    formFieldMessage(form.formState.errors.description, "description") ??
    formFieldMessage(form.formState.errors.icon, "icon");

  return (
    <form
      className="project-form"
      noValidate
      onSubmit={form.handleSubmit(async (values) => {
        setServerError(null);
        const parsed = projectCreatePayload(values);
        if (!parsed.ok) {
          if (parsed.issue.field === "key" && parsed.issue.code === "reserved") {
            form.setError("key", { message: "i18n:project.key.reserved" });
          } else if (parsed.issue.field === "key" && parsed.issue.code === "pattern") {
            form.setError("key", { message: "i18n:form.pattern.key" });
          } else if (parsed.issue.code === "too_big") {
            form.setError(parsed.issue.field, { message: "i18n:form.too_big" });
          } else {
            form.setError(parsed.issue.field, { message: "i18n:form.too_small" });
          }
          return;
        }
        try {
          await onSubmit(parsed.body);
        } catch (err) {
          setServerError(err instanceof ProblemError ? err.title : t("error.network"));
        }
      })}
    >
      <div className="project-form__row">
        <div className="project-form__field">
          <Label htmlFor="project-key">{t("project.keyLabel")}</Label>
          <Input
            id="project-key"
            placeholder="LAB"
            autoComplete="off"
            disabled={pending || form.formState.isSubmitting}
            {...form.register("key", {
              onChange: (event) => {
                form.setValue("key", canonicalizeProjectKey(event.target.value), {
                  shouldValidate: false,
                });
              },
            })}
          />
          <p className="project-form__hint">{t("form.pattern.key")}</p>
        </div>
        <div className="project-form__field">
          <Label htmlFor="project-name">{t("project.name")}</Label>
          <Input
            id="project-name"
            disabled={pending || form.formState.isSubmitting}
            {...form.register("name")}
          />
        </div>
        <div className="project-form__field">
          <Label htmlFor="project-icon">{t("project.icon")}</Label>
          <Input
            id="project-icon"
            disabled={pending || form.formState.isSubmitting}
            {...form.register("icon")}
          />
        </div>
        <div className="project-form__field">
          <Label htmlFor="project-visibility">{t("project.visibility")}</Label>
          <select
            id="project-visibility"
            disabled={pending || form.formState.isSubmitting}
            {...form.register("visibility")}
          >
            <option value="workspace">{t("project.visibility.workspaceAll")}</option>
            <option value="private">{t("project.visibility.private")}</option>
          </select>
        </div>
      </div>
      <div className="project-form__field">
        <Label htmlFor="project-description">{t("project.description")}</Label>
        <textarea
          id="project-description"
          rows={2}
          placeholder={t("project.description.placeholder")}
          disabled={pending || form.formState.isSubmitting}
          {...form.register("description")}
        />
      </div>
      {fieldError ? (
        <p role="alert" className="project-form__alert">
          {fieldError}
        </p>
      ) : null}
      {serverError ? (
        <p role="alert" className="project-form__alert">
          {serverError}
        </p>
      ) : null}
      <div className="project-form__actions">
        <Button type="submit" disabled={pending || form.formState.isSubmitting}>
          {pending || form.formState.isSubmitting ? t("project.create.pending") : t("project.new")}
        </Button>
        <Button type="button" variant="outline" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
      </div>
    </form>
  );
}
