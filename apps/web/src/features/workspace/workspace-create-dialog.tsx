// Adapted from fvoci/FVOCI apps/web/src/features/workspace/workspace-create-dialog.tsx
import { t, type I18nKey } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ProblemError } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import { workspaceCreateInput } from "@/lib/validators";
import "./workspace-aux.css";

export function WorkspaceCreateDialog({
  open,
  onOpenChange,
  onCreate,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreate: (input: { name: string; slug: string }) => Promise<void>;
}) {
  const form = useForm<{ name: string; slug: string }>({
    resolver: zodResolver(workspaceCreateInput),
    defaultValues: { name: "", slug: "" },
  });
  const fieldError =
    formFieldMessage(form.formState.errors.name, "name") ??
    formFieldMessage(form.formState.errors.slug, "slug");

  if (!open) return null;

  return (
    <div className="workspace-create" role="dialog" aria-modal="true">
      <h2 className="workspace-empty__heading">{t("workspace.create.dialog.title")}</h2>
      <p className="workspace-create__hint">{t("workspace.create.dialog.description")}</p>
      <form
        noValidate
        onSubmit={form.handleSubmit(async (values) => {
          try {
            await onCreate(values);
            form.reset();
            onOpenChange(false);
          } catch (error) {
            if (error instanceof ProblemError && (error.status === 400 || error.status === 409)) {
              form.setError("slug", {
                message: error.status === 409 ? "i18n:slug taken" : "i18n:form.invalid",
              });
            } else {
              form.setError("root", {
                message:
                  error instanceof ProblemError && error.titleKnown
                    ? error.title
                    : "i18n:error.workspace.create",
              });
            }
          }
        })}
      >
        <div className="workspace-create__field">
          <Label htmlFor="create-workspace-name">{t("workspace.create.name")}</Label>
          <Input id="create-workspace-name" autoFocus disabled={form.formState.isSubmitting} {...form.register("name")} />
        </div>
        <div className="workspace-create__field">
          <Label htmlFor="create-workspace-slug">{t("workspace.create.slug")}</Label>
          <Input id="create-workspace-slug" disabled={form.formState.isSubmitting} {...form.register("slug")} />
          <p className="workspace-create__hint">{t("form.pattern.slug")}</p>
        </div>
        {fieldError ? <p role="alert" className="workspace-create__alert">{fieldError}</p> : null}
        {form.formState.errors.root?.message ? (
          <p role="alert" className="workspace-create__alert">
            {form.formState.errors.root.message.startsWith("i18n:")
              ? t(form.formState.errors.root.message.slice(5) as I18nKey)
              : form.formState.errors.root.message}
          </p>
        ) : null}
        <div className="workspace-empty__actions">
          <Button type="submit" disabled={form.formState.isSubmitting}>{t("workspace.create.action")}</Button>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
        </div>
      </form>
    </div>
  );
}
