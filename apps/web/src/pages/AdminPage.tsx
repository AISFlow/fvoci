// Adapted from source apps/web/src/routes/settings.admin.tsx.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AdminShell } from "@/features/settings/admin-shell";
import { AdminSettingsView, adminActionMessage, type AdminUserPatch } from "@/features/settings/settings-admin";
import type { BrandingAssetKind } from "@/features/settings/settings-catalog";
import type { components } from "@/generated/api";
import { api, ensureOk } from "@/lib/api";
import {
  adminInstanceSettingsQuery,
  adminSystemQuery,
  adminUsersQuery,
  adminWorkspacesQuery,
  invalidateInstanceWrites,
} from "@/lib/queries/admin";

type SettingsPatch = components["schemas"]["InstanceSettingsPatchInput"];

function AdminConsole() {
  const queryClient = useQueryClient();
  const usersQuery = useQuery(adminUsersQuery);
  const workspacesQuery = useQuery(adminWorkspacesQuery);
  const systemQuery = useQuery(adminSystemQuery);
  const settingsQuery = useQuery(adminInstanceSettingsQuery);

  const saveSettings = useMutation({
    mutationFn: async (patch: Record<string, unknown>) =>
      ensureOk(
        await api.PATCH("/api/v1/admin/instance-settings", {
          // The catalog form builds the body key by key; the server checks it strictly.
          body: patch as SettingsPatch,
        }),
      ),
    onSuccess: (data) => {
      queryClient.setQueryData(adminInstanceSettingsQuery.queryKey, data);
      return invalidateInstanceWrites(queryClient);
    },
  });

  // Assets are not PATCH leaves: upload is a raw octet POST, clearing a DELETE.
  const saveAsset = useMutation({
    mutationFn: async ({ kind, file }: { kind: BrandingAssetKind; file: File | null }) =>
      file === null
        ? ensureOk(
            await api.DELETE("/api/v1/admin/branding/assets/{asset}", {
              params: { path: { asset: kind } },
            }),
          )
        : ensureOk(
            await api.POST("/api/v1/admin/branding/assets/{asset}", {
              params: { path: { asset: kind } },
              body: file as unknown as number[],
              bodySerializer: (body: unknown) => body as BodyInit,
              headers: { "Content-Type": "application/octet-stream" },
            }),
          ),
    onSuccess: (data) => {
      queryClient.setQueryData(adminInstanceSettingsQuery.queryKey, data);
      return invalidateInstanceWrites(queryClient);
    },
  });

  const patchUser = useMutation({
    mutationFn: async (input: { userId: string } & AdminUserPatch) =>
      ensureOk(await api.PATCH("/api/v1/admin/users", { body: input })),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["admin"] }),
  });

  const err = usersQuery.error ?? workspacesQuery.error ?? systemQuery.error;
  const settingsError = settingsQuery.error ?? saveSettings.error ?? saveAsset.error;

  return (
    <AdminSettingsView
      users={usersQuery.data?.items ?? []}
      workspaces={workspacesQuery.data?.items ?? []}
      system={systemQuery.data ?? null}
      loading={usersQuery.isLoading || workspacesQuery.isLoading || systemQuery.isLoading}
      error={err ? adminActionMessage(err) : null}
      pending={patchUser.isPending}
      onPatchUser={(userId, patch) => patchUser.mutateAsync({ userId, ...patch }).then(() => undefined)}
      settings={settingsQuery.data ?? null}
      settingsLoading={settingsQuery.isLoading}
      settingsError={settingsError ? adminActionMessage(settingsError) : null}
      onRetrySettings={
        settingsQuery.error
          ? () => {
              void settingsQuery.refetch();
            }
          : undefined
      }
      settingsPending={saveSettings.isPending || saveAsset.isPending}
      onSaveSettings={(patch) => {
        saveAsset.reset();
        return saveSettings.mutateAsync(patch).then(() => undefined);
      }}
      onSettingsAsset={(kind, file) => {
        saveSettings.reset();
        return saveAsset.mutateAsync({ kind, file }).then(() => undefined);
      }}
    />
  );
}

export function AdminPage() {
  return (
    <AdminShell active="admin">
      <AdminConsole />
    </AdminShell>
  );
}
