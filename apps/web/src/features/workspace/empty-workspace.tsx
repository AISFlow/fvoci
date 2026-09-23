// Adapted from fvoci/FVOCI apps/web/src/features/workspace/empty-workspace.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ProblemError } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import { workspaceCreateInput } from "@/lib/validators";
import "./workspace-aux.css";

export function EmptyWorkspace(props: {
  isAdmin: boolean;
  onCreate: (input: { name: string; slug: string }) => Promise<void>;
  onLogout: () => void;
  error?: string | null;
  onRetry?: () => void;
}) {
  const [createError, setCreateError] = useState<string | null>(null);
  const form = useForm<{ name: string; slug: string }>({
    resolver: zodResolver(workspaceCreateInput),
    defaultValues: { name: "", slug: "" },
  });
  const fieldError =
    formFieldMessage(form.formState.errors.name, "name") ??
    formFieldMessage(form.formState.errors.slug, "slug");

  if (props.error) {
    return (
      <div className="workspace-empty">
        <p role="alert" className="workspace-empty__lead">{props.error}</p>
        <Button type="button" size="sm" className="w-fit" onClick={props.onRetry}>{t("load.retry")}</Button>
      </div>
    );
  }

  const logout = (
    <Button type="button" variant="outline" size="sm" className="w-fit" onClick={props.onLogout}>
      {t("nav.logout")}
    </Button>
  );

  if (!props.isAdmin) {
    return (
      <div className="workspace-empty">
        <p className="workspace-empty__lead">{t("workspace.none.invite")}</p>
        <div className="workspace-empty__actions">{logout}</div>
      </div>
    );
  }

  return (
    <form
      className="workspace-empty"
      onSubmit={form.handleSubmit(async (values) => {
        setCreateError(null);
        try {
          await props.onCreate(values);
        } catch (err) {
          if (err instanceof ProblemError && err.status === 403) {
            setCreateError(t("unauthorized"));
          } else if (err instanceof ProblemError && (err.status === 400 || err.status === 409)) {
            form.setError("slug", {
              message: err.status === 409 ? "i18n:slug taken" : "i18n:form.invalid",
            });
          } else if (err instanceof ProblemError) {
            setCreateError(err.titleKnown ? err.title : t("error.workspace.create"));
          } else {
            setCreateError(t("error.network"));
          }
        }
      })}
      noValidate
    >
      <h1 className="workspace-empty__heading">{t("workspace.none")}</h1>
      <div className="workspace-create__field">
        <Label htmlFor="ws-name">{t("workspace.create.name")}</Label>
        <Input id="ws-name" disabled={form.formState.isSubmitting} {...form.register("name")} />
      </div>
      <div className="workspace-create__field">
        <Label htmlFor="ws-slug">{t("workspace.create.slug")}</Label>
        <Input id="ws-slug" disabled={form.formState.isSubmitting} {...form.register("slug")} />
        <p className="workspace-create__hint">{t("form.pattern.slug")}</p>
      </div>
      {fieldError ? <p role="alert" className="workspace-create__alert">{fieldError}</p> : null}
      {createError ? <p role="alert" className="workspace-create__alert">{createError}</p> : null}
      <div className="workspace-empty__actions">
        <Button type="submit" size="sm" disabled={form.formState.isSubmitting}>{t("workspace.create")}</Button>
        {logout}
      </div>
    </form>
  );
}
