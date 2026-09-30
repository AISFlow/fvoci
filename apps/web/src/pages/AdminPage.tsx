// Adapted from source apps/web/src/routes/settings.admin.tsx.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  type AdminUserPatch,
  cancelAdminUserErasure,
  eraseAdminUser,
  patchAdminUser,
  patchInstanceSettings,
  saveBrandingAsset,
} from "@/features/settings/admin-requests";
import { adminActionMessage } from "@/features/settings/admin-users";
import { AdminShell } from "@/features/settings/admin-shell";
import { AdminSettingsView } from "@/features/settings/settings-admin";
import type { BrandingAssetKind } from "@/features/settings/settings-catalog";
import {
  adminInstanceSettingsQuery,
  adminSystemQuery,
  adminUsersQuery,
  adminWorkspacesQuery,
  invalidateInstanceWrites,
} from "@/lib/queries/admin";

function AdminConsole() {
  const queryClient = useQueryClient();
  const usersQuery = useQuery(adminUsersQuery);
  const workspacesQuery = useQuery(adminWorkspacesQuery);
  const systemQuery = useQuery(adminSystemQuery);
  const settingsQuery = useQuery(adminInstanceSettingsQuery);

  const saveSettings = useMutation({
    mutationFn: patchInstanceSettings,
    onSuccess: (data) => {
      queryClient.setQueryData(adminInstanceSettingsQuery.queryKey, data);
      return invalidateInstanceWrites(queryClient);
    },
  });

  // Assets are not PATCH leaves: upload is a raw octet POST, clearing a DELETE.
  const saveAsset = useMutation({
    mutationFn: ({ kind, file }: { kind: BrandingAssetKind; file: File | null }) =>
      saveBrandingAsset(kind, file),
    onSuccess: (data) => {
      queryClient.setQueryData(adminInstanceSettingsQuery.queryKey, data);
      return invalidateInstanceWrites(queryClient);
    },
  });

  const patchUser = useMutation({
    mutationFn: (input: { userId: string } & AdminUserPatch) => patchAdminUser(input),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["admin"] }),
  });

  // Source eraseUser / cancelEraseUser mutations.
  const eraseUser = useMutation({
    mutationFn: eraseAdminUser,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["admin"] }),
  });
  const cancelEraseUser = useMutation({
    mutationFn: cancelAdminUserErasure,
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
      pending={patchUser.isPending || eraseUser.isPending || cancelEraseUser.isPending}
      onPatchUser={(userId, patch) => patchUser.mutateAsync({ userId, ...patch }).then(() => undefined)}
      onEraseUser={(userId) => eraseUser.mutateAsync(userId).then(() => undefined)}
      onCancelEraseUser={(userId) => cancelEraseUser.mutateAsync(userId).then(() => undefined)}
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
