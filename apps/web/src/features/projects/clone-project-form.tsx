import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ProblemError } from "@/lib/api";
import { canonicalizeProjectKey } from "@/lib/href";
import { LeadSelect } from "./lead-select";
import {
  PROJECT_DESCRIPTION_MAX,
  PROJECT_ICON_MAX,
  PROJECT_NAME_MAX,
  projectCreatePayload,
} from "./create-payload";
import type { CloneProjectBody, ProjectListItem } from "./queries";
import type { components } from "@/generated/api";

type Member = components["schemas"]["MemberResponse"];

const cloneSchema = z.object({
  key: z.string().min(1, "i18n:form.too_small"),
  name: z.string().trim().min(1, "i18n:form.too_small").max(PROJECT_NAME_MAX, "i18n:form.too_big"),
  visibility: z.enum(["workspace", "private"]),
  description: z.string().max(PROJECT_DESCRIPTION_MAX, "i18n:form.too_big").optional(),
  icon: z.string().max(PROJECT_ICON_MAX, "i18n:form.too_big").optional(),
  leadUserId: z.string().optional(),
});

type CloneForm = z.infer<typeof cloneSchema>;

export function CloneProjectForm({
  source,
  pending,
  members,
  currentUserId,
  onSubmit,
  onCancel,
}: {
  source: ProjectListItem;
  pending?: boolean;
  members: readonly Member[];
  currentUserId: string | null;
  onSubmit: (input: CloneProjectBody) => Promise<void>;
  onCancel: () => void;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const defaultName =
    source.name.length + " (복사)".length > PROJECT_NAME_MAX
      ? source.name.slice(0, PROJECT_NAME_MAX - " (복사)".length) + " (복사)"
      : `${source.name} (복사)`;
  const form = useForm<CloneForm>({
    resolver: zodResolver(cloneSchema),
    defaultValues: {
      key: "",
      name: defaultName,
      visibility: source.visibility === "private" ? "private" : "workspace",
      description: source.description ?? "",
      icon: source.icon ?? "",
      leadUserId: currentUserId ?? undefined,
    },
  });

  return (
    <form
      className="project-form"
      noValidate
      onSubmit={form.handleSubmit(async (values) => {
        setServerError(null);
        const parsed = projectCreatePayload(values);
        if (!parsed.ok) {
          form.setError(parsed.issue.field, { message: `i18n:form.${parsed.issue.code}` });
          return;
        }
        const body: CloneProjectBody = {
          key: parsed.body.key,
          name: parsed.body.name,
          visibility: parsed.body.visibility,
          description: parsed.body.description,
          icon: parsed.body.icon,
          leadUserId: values.leadUserId,
        };
        try {
          await onSubmit(body);
        } catch (err) {
          setServerError(err instanceof ProblemError ? err.title : t("error.network"));
        }
      })}
    >
      <p className="project-form__hint">{t("project.clone.help")}</p>
      <p className="project-form__hint">
        {t("project.clone.source")}: {source.key} — {source.name}
      </p>
      <div className="project-form__row">
        <div className="project-form__field">
          <Label htmlFor="clone-key">{t("project.keyLabel")}</Label>
          <Input
            id="clone-key"
            autoComplete="off"
            disabled={pending || form.formState.isSubmitting}
            {...form.register("key", {
              onChange: (event) => {
                form.setValue("key", canonicalizeProjectKey(event.target.value));
              },
            })}
          />
        </div>
        <div className="project-form__field">
          <Label htmlFor="clone-name">{t("project.name")}</Label>
          <Input id="clone-name" disabled={pending || form.formState.isSubmitting} {...form.register("name")} />
        </div>
        <LeadSelect
          id="clone-lead"
          value={form.watch("leadUserId")}
          members={members}
          disabled={pending || form.formState.isSubmitting}
          onChange={(userId) => form.setValue("leadUserId", userId)}
        />
      </div>
      {serverError ? (
        <p role="alert" className="project-form__alert">{serverError}</p>
      ) : null}
      <div className="project-form__actions">
        <Button type="submit" disabled={pending || form.formState.isSubmitting}>
          {pending || form.formState.isSubmitting ? t("project.clone.pending") : t("project.clone")}
        </Button>
        <Button type="button" variant="outline" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
      </div>
    </form>
  );
}
