<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref } from "vue";
import ConfirmAction from "../../components/ConfirmAction.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import type { AdminUserPatch } from "@/features/settings/admin-requests";
import { adminActionMessage, daysUntil } from "@/features/settings/admin-users";
import type { BrandingAssetKind } from "@/features/settings/settings-catalog";
import type { components } from "@/generated/api";
import { formatDateKo } from "@/lib/datetime";
import InstanceSettingsView from "./InstanceSettingsView.vue";
import { tableCellClass, tableHeadClass } from "./field-classes";
import "@/features/settings/settings-shell.css";

type AdminUser = components["schemas"]["AdminUserItemOutput"];
type AdminWorkspace = components["schemas"]["AdminWorkspaceItemOutput"];
type AdminSystem = components["schemas"]["AdminSystemOutput"];
type AdminInstanceSettings = components["schemas"]["AdminInstanceSettingsOutput"];

const props = defineProps<{
  users: AdminUser[];
  workspaces: AdminWorkspace[];
  system: AdminSystem | null;
  loading: boolean;
  error: string | null;
  pending: boolean;
  onPatchUser: (userId: string, patch: AdminUserPatch) => Promise<void>;
  onEraseUser: (userId: string) => Promise<void>;
  onCancelEraseUser: (userId: string) => Promise<void>;
  settings: AdminInstanceSettings | null;
  settingsPending: boolean;
  settingsLoading?: boolean;
  settingsError?: string | null;
  onRetrySettings?: () => void;
  onSaveSettings: (patch: Record<string, unknown>) => Promise<void>;
  onSettingsAsset: (kind: BrandingAssetKind, file: File | null) => Promise<void>;
}>();

const actionError = ref<string | null>(null);
const changing = ref(false);
const busy = () => props.pending || changing.value;

async function runUserAction(task: () => Promise<void>): Promise<void> {
  if (busy()) return;
  actionError.value = null;
  changing.value = true;
  try {
    await task();
  } catch (err) {
    actionError.value = adminActionMessage(err);
  } finally {
    changing.value = false;
  }
}

function patchUser(userId: string, patch: AdminUserPatch): Promise<void> {
  return runUserAction(() => props.onPatchUser(userId, patch));
}

function pendingDays(user: AdminUser): number | null {
  return user.eraseAt ? daysUntil(user.eraseAt) : null;
}
</script>

<template>
  <div class="settings-stack">
    <section class="settings-section" aria-labelledby="admin-system-title">
      <h2 class="settings-section__title text-title" id="admin-system-title">{{ t("admin.system") }}</h2>
      <div class="text-sm">
        <QueryLoading v-if="loading" />
        <ul v-if="system" class="flex flex-col gap-1 settings-tabular">
          <li>{{ t("admin.users") }}: {{ system.users }}</li>
          <li>{{ t("admin.workspaces") }}: {{ system.workspaces }}</li>
          <li>{{ t("admin.documents") }}: {{ system.documents }}</li>
          <li>{{ t("admin.tasks") }}: {{ system.tasks }}</li>
        </ul>
        <p v-if="error" role="alert" class="text-error">{{ error }}</p>
      </div>
    </section>
    <section class="settings-section" aria-labelledby="admin-users-title">
      <h2 class="settings-section__title text-title" id="admin-users-title">{{ t("admin.users") }}</h2>
      <div class="flex flex-col gap-2 overflow-x-auto">
        <p v-if="actionError" role="alert" class="settings-notice settings-notice--danger">{{ actionError }}</p>
        <table class="w-full border-collapse">
          <thead>
            <tr>
              <th :class="tableHeadClass">{{ t("admin.email") }}</th>
              <th :class="tableHeadClass">{{ t("admin.instanceAdmin") }}</th>
              <th :class="tableHeadClass">{{ t("admin.suspended") }}</th>
              <th :class="tableHeadClass" />
            </tr>
          </thead>
          <tbody>
            <tr v-for="u in users" :key="u.id">
              <td :class="tableCellClass">
                {{ formatPersonName(u) }} ({{ u.email }})
                <p
                  v-if="u.eraseAt && pendingDays(u) !== null"
                  class="break-keep text-sm text-muted"
                >
                  {{
                    pendingDays(u) === 0
                      ? t("admin.erase.processing")
                      : t("admin.erase.until", { date: formatDateKo(u.eraseAt), days: pendingDays(u) })
                  }}
                </p>
              </td>
              <td :class="tableCellClass">{{ u.instanceAdmin ? t("admin.yes") : t("admin.no") }}</td>
              <td :class="tableCellClass">{{ u.suspendedAt ? t("admin.yes") : t("admin.no") }}</td>
              <td :class="[tableCellClass, 'text-right']">
                <div class="flex flex-wrap justify-end gap-2">
                  <UButton
                    type="button"
                    variant="outline"
                    color="neutral"
                    size="sm"
                    :aria-pressed="u.instanceAdmin"
                    :aria-label="`${t('admin.instanceAdmin')}: ${u.email}`"
                    :disabled="busy() || u.deletedAt != null"
                    @click="patchUser(u.id, { instanceAdmin: !u.instanceAdmin })"
                  >
                    {{ t("admin.instanceAdmin") }}
                  </UButton>
                  <ConfirmAction
                    :title="u.suspendedAt ? t('admin.restore.confirm.title') : t('admin.suspend.confirm.title')"
                    :description="u.suspendedAt ? t('admin.restore.confirm.body') : t('admin.suspend.confirm.body')"
                    :action-label="u.suspendedAt ? t('admin.restore') : t('admin.suspend')"
                    :disabled="busy() || u.deletedAt != null"
                    :destructive="!u.suspendedAt"
                    :on-confirm="() => patchUser(u.id, { suspended: !u.suspendedAt })"
                  >
                    {{ u.suspendedAt ? t("admin.restore") : t("admin.suspend") }}
                  </ConfirmAction>
                  <ConfirmAction
                    v-if="u.deletedAt != null"
                    :title="t('admin.erase.cancel.confirm.title', { name: formatPersonName(u) })"
                    :description="
                      u.suspendedAt
                        ? t('admin.erase.cancel.confirm.body.suspended')
                        : t('admin.erase.cancel.confirm.body')
                    "
                    :action-label="t('admin.erase.cancel')"
                    :disabled="busy() || pendingDays(u) === null || pendingDays(u) === 0"
                    :destructive="false"
                    :on-confirm="() => runUserAction(() => onCancelEraseUser(u.id))"
                  >
                    {{ t("admin.erase.cancel") }}
                  </ConfirmAction>
                  <ConfirmAction
                    v-else
                    :title="t('admin.erase.confirm.title', { name: formatPersonName(u) })"
                    :description="t('admin.erase.confirm.body', { name: formatPersonName(u) })"
                    :action-label="t('admin.erase')"
                    :disabled="busy()"
                    :on-confirm="() => runUserAction(() => onEraseUser(u.id))"
                  >
                    {{ t("admin.erase") }}
                  </ConfirmAction>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
    <section class="settings-section" aria-labelledby="admin-workspaces-title">
      <h2 class="settings-section__title text-title" id="admin-workspaces-title">{{ t("admin.workspaces") }}</h2>
      <div class="overflow-x-auto">
        <table class="w-full border-collapse">
          <thead>
            <tr>
              <th :class="tableHeadClass">{{ t("workspace.name") }}</th>
              <th :class="tableHeadClass">{{ t("admin.slug") }}</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="w in workspaces" :key="w.id">
              <td :class="tableCellClass">{{ w.name }}</td>
              <td :class="[tableCellClass, 'settings-tabular']">{{ w.slug }}</td>
            </tr>
          </tbody>
        </table>
      </div>
    </section>
    <InstanceSettingsView
      :data="settings"
      :error="settingsError ?? null"
      :loading="settingsLoading ?? false"
      :on-retry="onRetrySettings"
      :on-asset="onSettingsAsset"
      :on-save="onSaveSettings"
      :pending="settingsPending"
    />
  </div>
</template>
