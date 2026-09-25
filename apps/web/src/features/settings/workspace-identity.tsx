// Name/identity section adapted from settings-workspace.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useEffect, useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { formFieldMessage } from "@/lib/form-issues";
import { workspaceDeleteInput, workspaceNameInput } from "@/lib/validators";
import "../settings/settings-shell.css";

export function WorkspaceIdentitySection({
  workspaceName,
  workspaceSlug,
  workspaceKind,
  canManage,
  isOwner,
  namePending,
  nameError,
  onSaveName,
  deletePending,
  deleteError,
  onDelete,
}: {
  workspaceName: string;
  workspaceSlug: string;
  workspaceKind: string;
  canManage: boolean;
  isOwner: boolean;
  namePending: boolean;
  nameError: string | null;
  onSaveName: (name: string) => Promise<void>;
  deletePending: boolean;
  deleteError: string | null;
  onDelete: (confirmSlug: string) => Promise<void>;
}) {
  const [nameSaved, setNameSaved] = useState(false);
  const nameForm = useForm<{ name: string }>({
    resolver: zodResolver(workspaceNameInput),
    defaultValues: { name: workspaceName },
  });
  const deleteForm = useForm<{ confirmSlug: string }>({
    resolver: zodResolver(workspaceDeleteInput),
    defaultValues: { confirmSlug: "" },
  });
  const nameDirty = nameForm.formState.isDirty;

  useEffect(() => {
    if (!nameDirty) nameForm.reset({ name: workspaceName });
  }, [workspaceName, nameDirty, nameForm]);

  useEffect(() => {
    if (nameDirty) setNameSaved(false);
  }, [nameDirty]);

  const nameFieldError = formFieldMessage(nameForm.formState.errors.name, "name");
  const confirmSlugError = formFieldMessage(
    deleteForm.formState.errors.confirmSlug,
    "confirmSlug",
  );

  return (
    <section className="settings-section">
      <h1 className="settings-section__title text-title">{t("settings.workspace")}</h1>
      {workspaceName !== "" && !(workspaceKind === "team" && canManage) ? (
        <p className="text-ui font-medium break-keep">{workspaceName}</p>
      ) : null}
      {workspaceSlug !== "" ? (
        <p className="settings-section__lede">
          {t("workspace.settings.slug")} <span className="settings-tabular">{workspaceSlug}</span>
        </p>
      ) : null}
      {workspaceKind === "personal" ? (
        <p className="text-ui text-muted-foreground">{t("personal workspace is immutable")}</p>
      ) : null}
      {workspaceKind === "team" && canManage ? (
        <form
          onSubmit={nameForm.handleSubmit(async (values) => {
            setNameSaved(false);
            try {
              await onSaveName(values.name);
              nameForm.reset({ name: values.name });
              setNameSaved(true);
            } catch {
              return;
            }
          })}
          noValidate
          className="settings-form"
        >
          <Label htmlFor="workspace-name">{t("workspace.name")}</Label>
          <div className="settings-form__row">
            <Input
              id="workspace-name"
              disabled={namePending}
              aria-invalid={nameFieldError || nameError ? true : undefined}
              className="h-11 min-w-0 flex-1 sm:min-w-40"
              {...nameForm.register("name")}
            />
            <Button type="submit" size="sm" disabled={namePending}>{t("workspace.save")}</Button>
          </div>
          {nameFieldError ? (
            <p role="alert" className="settings-notice settings-notice--danger">{nameFieldError}</p>
          ) : null}
          {nameError ? (
            <p role="alert" className="settings-notice settings-notice--danger">{nameError}</p>
          ) : null}
          {nameSaved && !nameError ? (
            <p role="status" className="settings-notice settings-notice--ok">{t("workspace.settings.saved")}</p>
          ) : null}
        </form>
      ) : null}
      {workspaceKind === "team" && !canManage ? (
        <p className="text-ui text-muted-foreground">{t("workspace.settings.readOnly")}</p>
      ) : null}
      {workspaceKind === "team" && isOwner ? (
        <details className="settings-disclosure">
          <summary className="settings-disclosure__summary">{t("workspace.delete")}</summary>
          <div className="settings-disclosure__body">
            <form
              onSubmit={deleteForm.handleSubmit(async (values) => {
                try {
                  await onDelete(values.confirmSlug);
                } catch {
                  return;
                }
              })}
              noValidate
              className="settings-form"
            >
              <Label htmlFor="workspace-delete-confirm">{t("workspace.deleteConfirm")}</Label>
              <Input
                id="workspace-delete-confirm"
                type="text"
                autoComplete="off"
                disabled={deletePending}
                aria-invalid={confirmSlugError || deleteError ? true : undefined}
                aria-describedby={
                  confirmSlugError ? "workspace-delete-confirm-error" : undefined
                }
                {...deleteForm.register("confirmSlug")}
              />
              {confirmSlugError ? (
                <p
                  id="workspace-delete-confirm-error"
                  role="alert"
                  className="settings-notice settings-notice--danger"
                >
                  {confirmSlugError}
                </p>
              ) : null}
              {deleteError ? (
                <p role="alert" className="settings-notice settings-notice--danger">
                  {deleteError}
                </p>
              ) : null}
              <Button type="submit" size="sm" variant="destructive" disabled={deletePending}>
                {t("workspace.delete")}
              </Button>
            </form>
          </div>
        </details>
      ) : null}
    </section>
  );
}
