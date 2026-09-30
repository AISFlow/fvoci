<script setup lang="ts">
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { meQuery } from "@/lib/queries";
import {
  type AdminUserPatch,
  cancelAdminUserErasure,
  eraseAdminUser,
  patchAdminUser,
  patchInstanceSettings,
  saveBrandingAsset,
} from "@/features/settings/admin-requests";
import { adminActionMessage } from "@/features/settings/admin-users";
import type { BrandingAssetKind } from "@/features/settings/settings-catalog";
import {
  adminInstanceSettingsQuery,
  adminSystemQuery,
  adminUsersQuery,
  adminWorkspacesQuery,
  invalidateInstanceWrites,
} from "@/lib/queries/admin";
import AdminShell from "../components/AdminShell.vue";
import AdminSettingsView from "../features/settings/AdminSettingsView.vue";

const queryClient = useQueryClient();
const me = useQuery(meQuery);
const usersQuery = useQuery(() => ({
  ...adminUsersQuery,
  enabled: me.data.value?.isInstanceAdmin === true,
}));
const workspacesQuery = useQuery(() => ({
  ...adminWorkspacesQuery,
  enabled: me.data.value?.isInstanceAdmin === true,
}));
const systemQuery = useQuery(() => ({
  ...adminSystemQuery,
  enabled: me.data.value?.isInstanceAdmin === true,
}));
const settingsQuery = useQuery(() => ({
  ...adminInstanceSettingsQuery,
  enabled: me.data.value?.isInstanceAdmin === true,
}));

const saveSettings = useMutation({
  mutationFn: patchInstanceSettings,
  onSuccess: (data) => {
    queryClient.setQueryData(adminInstanceSettingsQuery.queryKey, data);
    return invalidateInstanceWrites(queryClient);
  },
});

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

const eraseUser = useMutation({
  mutationFn: eraseAdminUser,
  onSuccess: () => queryClient.invalidateQueries({ queryKey: ["admin"] }),
});
const cancelEraseUser = useMutation({
  mutationFn: cancelAdminUserErasure,
  onSuccess: () => queryClient.invalidateQueries({ queryKey: ["admin"] }),
});

const err = () => usersQuery.error.value ?? workspacesQuery.error.value ?? systemQuery.error.value;
const settingsError = () =>
  settingsQuery.error.value ?? saveSettings.error.value ?? saveAsset.error.value;
</script>

<template>
  <AdminShell active="admin">
    <AdminSettingsView
      :users="usersQuery.data.value?.items ?? []"
      :workspaces="workspacesQuery.data.value?.items ?? []"
      :system="systemQuery.data.value ?? null"
      :loading="
        usersQuery.isLoading.value || workspacesQuery.isLoading.value || systemQuery.isLoading.value
      "
      :error="err() ? adminActionMessage(err()) : null"
      :pending="
        patchUser.isPending.value || eraseUser.isPending.value || cancelEraseUser.isPending.value
      "
      :on-patch-user="
        (userId, patch) => patchUser.mutateAsync({ userId, ...patch }).then(() => undefined)
      "
      :on-erase-user="(userId) => eraseUser.mutateAsync(userId).then(() => undefined)"
      :on-cancel-erase-user="(userId) => cancelEraseUser.mutateAsync(userId).then(() => undefined)"
      :settings="settingsQuery.data.value ?? null"
      :settings-loading="settingsQuery.isLoading.value"
      :settings-error="settingsError() ? adminActionMessage(settingsError()) : null"
      :on-retry-settings="
        settingsQuery.error.value
          ? () => {
              void settingsQuery.refetch();
            }
          : undefined
      "
      :settings-pending="saveSettings.isPending.value || saveAsset.isPending.value"
      :on-save-settings="
        (patch) => {
          saveAsset.reset();
          return saveSettings.mutateAsync(patch).then(() => undefined);
        }
      "
      :on-settings-asset="
        (kind, file) => {
          saveSettings.reset();
          return saveAsset.mutateAsync({ kind, file }).then(() => undefined);
        }
      "
    />
  </AdminShell>
</template>
