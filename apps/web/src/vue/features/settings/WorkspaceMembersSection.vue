<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { MemberOutput, WorkspaceRole } from "@/lib/contracts";
import { membersQuery } from "@/lib/queries";
import { invitationCreateInput } from "@/lib/validators";
import ConfirmDialog from "./ConfirmDialog.vue";
import { copyText } from "./clipboard";
import { parseForm } from "./form";
import { canManageMember, inviteRolesFor, roleAtLeast, roleLabel } from "./workspace-role";
import "@/features/settings/settings-shell.css";

const props = defineProps<{
  workspaceId: string;
  currentUserId: string | null;
  currentUserRole: string;
}>();

const queryClient = useQueryClient();
const members = useQuery(() => membersQuery(props.workspaceId));
const canManage = computed(() => roleAtLeast(props.currentUserRole, "admin"));
const inviteRoles = computed(() => inviteRolesFor(props.currentUserRole));
const items = computed(() => members.data.value?.items ?? []);
const membersError = computed(() =>
  members.error.value instanceof ProblemError
    ? members.error.value.title
    : members.error.value
      ? t("error.network")
      : null,
);

const email = ref("");
const inviteRole = ref<WorkspaceRole>("member");
const emailError = ref<string | null>(null);
const inviteError = ref<string | null>(null);
const inviteResult = ref<{ acceptUrl: string; mailDelayed?: boolean | null } | null>(null);
const inviteCopyStatus = ref<"copied" | "failed" | null>(null);
const pendingIds = ref<ReadonlySet<string>>(new Set());
const rowErrors = ref<Record<string, string>>({});
const status = ref<string | null>(null);
const removeTarget = ref<MemberOutput | null>(null);
const removeError = ref<string | null>(null);

const invite = useMutation({
  mutationFn: async (input: { email: string; role: WorkspaceRole }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/invitations", {
        params: { path: { workspace_id: props.workspaceId } },
        body: input,
      }),
    ),
});

const patchRole = useMutation({
  mutationFn: async (input: { userId: string; role: WorkspaceRole }) =>
    ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}/members/{user_id}", {
        params: { path: { workspace_id: props.workspaceId, user_id: input.userId } },
        body: { role: input.role },
      }),
    ),
  onSuccess: async () => {
    await queryClient.invalidateQueries({ queryKey: ["workspaces", props.workspaceId, "members"] });
  },
});

const removeMember = useMutation({
  mutationFn: async (userId: string) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/members/{user_id}", {
        params: { path: { workspace_id: props.workspaceId, user_id: userId } },
      }),
    ),
  onSuccess: async () => {
    await queryClient.invalidateQueries({ queryKey: ["workspaces", props.workspaceId, "members"] });
  },
});

async function run(userId: string, action: () => Promise<void>): Promise<string | null> {
  pendingIds.value = new Set(pendingIds.value).add(userId);
  const nextErrors = { ...rowErrors.value };
  delete nextErrors[userId];
  rowErrors.value = nextErrors;
  status.value = null;
  try {
    await action();
    return null;
  } catch (err) {
    const message = err instanceof ProblemError ? err.title : t("workspace.member.action.failed");
    rowErrors.value = { ...rowErrors.value, [userId]: message };
    return message;
  } finally {
    const next = new Set(pendingIds.value);
    next.delete(userId);
    pendingIds.value = next;
  }
}

function memberName(member: MemberOutput): string {
  return formatPersonName({ givenName: member.givenName, familyName: member.familyName });
}

function onInvite(): void {
  emailError.value = null;
  inviteError.value = null;
  inviteResult.value = null;
  inviteCopyStatus.value = null;
  const parsed = parseForm(invitationCreateInput, { email: email.value, role: inviteRole.value });
  if (!parsed.ok) {
    emailError.value = parsed.message;
    return;
  }
  void invite.mutateAsync(parsed.data).then(
    (created) => {
      inviteResult.value = created;
      email.value = "";
    },
    (err: unknown) => {
      inviteError.value = err instanceof ProblemError ? err.title : t("error.network");
    },
  );
}

function onRoleChange(member: MemberOutput, event: Event): void {
  const next = (event.target as HTMLSelectElement).value as WorkspaceRole;
  if (next === member.role) return;
  const name = memberName(member);
  void run(member.userId, async () => {
    await patchRole.mutateAsync({ userId: member.userId, role: next });
    status.value = t("workspace.member.roleChanged", { name });
  });
}

function confirmRemove(): void {
  const member = removeTarget.value;
  if (!member) return;
  const name = memberName(member);
  void run(member.userId, async () => {
    await removeMember.mutateAsync(member.userId);
    status.value = t("workspace.member.removed", { name });
    removeTarget.value = null;
    removeError.value = null;
  }).then((error) => {
    if (error) removeError.value = error;
  });
}

function copyInvite(url: string): void {
  void copyText(url).then(
    () => {
      inviteCopyStatus.value = "copied";
    },
    () => {
      inviteCopyStatus.value = "failed";
    },
  );
}
</script>

<template>
  <details class="settings-disclosure">
    <summary class="settings-disclosure__summary">{{ t("workspace.members") }}</summary>
    <div class="settings-disclosure__body">
      <p v-if="members.isPending.value" role="status">{{ t("load.loading") }}</p>
      <ul v-if="items.length > 0" class="flex flex-col divide-y">
        <li
          v-for="member in items"
          :key="member.userId"
          class="flex min-w-0 flex-col gap-2 py-3 first:pt-0 last:pb-0 sm:flex-row sm:items-center sm:justify-between"
        >
          <div class="min-w-0">
            <p class="font-medium break-keep">
              {{ memberName(member) }}
              <span v-if="member.userId === currentUserId" class="ml-2 text-muted">{{ t("workspace.member.self") }}</span>
            </p>
            <p class="text-muted wrap-anywhere">{{ member.email }}</p>
            <p v-if="rowErrors[member.userId]" role="alert" class="mt-1 text-error break-keep">
              {{ rowErrors[member.userId] }}
            </p>
          </div>
          <div class="flex min-w-0 flex-wrap items-center gap-2 sm:justify-end">
            <template
              v-if="
                canManageMember({
                  currentUserRole,
                  currentUserId,
                  memberUserId: member.userId,
                  memberRole: member.role,
                })
              "
            >
              <label class="sr-only" :for="`member-role-${member.userId}`">
                {{ t("workspace.member.roleLabel", { name: memberName(member) }) }}
              </label>
              <select
                :id="`member-role-${member.userId}`"
                class="h-11 min-w-28 rounded-md border border-default bg-default px-3"
                :aria-label="t('workspace.member.roleLabel', { name: memberName(member) })"
                :value="member.role"
                :disabled="pendingIds.has(member.userId)"
                @change="onRoleChange(member, $event)"
              >
                <option v-for="role in inviteRoles" :key="role" :value="role">{{ roleLabel(role) }}</option>
              </select>
              <UButton
                type="button"
                size="sm"
                variant="outline"
                color="neutral"
                :disabled="pendingIds.has(member.userId)"
                @click="removeTarget = member"
              >
                {{ t("workspace.member.remove.action") }}
              </UButton>
            </template>
            <span v-else class="text-muted">{{ roleLabel(member.role) }}</span>
          </div>
        </li>
      </ul>
      <p v-if="status" role="status">{{ status }}</p>
      <p v-if="membersError" role="alert" class="settings-notice settings-notice--danger">{{ membersError }}</p>
      <form v-if="canManage" class="flex flex-col gap-1.5" novalidate @submit.prevent="onInvite">
        <div class="flex flex-wrap items-end gap-2">
          <div class="flex min-w-40 flex-1 flex-col gap-1.5">
            <label for="workspace-invite-email">{{ t("workspace.invite.email") }}</label>
            <UInput
              id="workspace-invite-email"
              v-model="email"
              type="email"
              autocomplete="email"
              :disabled="invite.isPending.value"
              :aria-invalid="emailError ? true : undefined"
            />
          </div>
          <div class="flex flex-col gap-1.5">
            <label for="workspace-invite-role">{{ t("group.role") }}</label>
            <select
              id="workspace-invite-role"
              v-model="inviteRole"
              class="h-11 min-w-28 rounded-md border border-default bg-default px-3"
              :disabled="invite.isPending.value"
            >
              <option v-for="role in inviteRoles" :key="role" :value="role">{{ roleLabel(role) }}</option>
            </select>
          </div>
          <UButton type="submit" size="sm" :disabled="invite.isPending.value">{{ t("auth.invite.send") }}</UButton>
        </div>
        <p v-if="emailError" role="alert" class="settings-notice settings-notice--danger">{{ emailError }}</p>
        <p v-if="inviteError" role="alert" class="settings-notice settings-notice--danger">{{ inviteError }}</p>
        <div v-if="inviteResult" class="flex flex-col gap-2">
          <p role="status">
            {{ t("workspace.invite.created") }}
            <template v-if="inviteResult.mailDelayed"> {{ t("workspace.invite.mailDelayed") }}</template>
          </p>
          <p class="wrap-anywhere">
            <a :href="inviteResult.acceptUrl" class="underline underline-offset-2">{{ inviteResult.acceptUrl }}</a>
          </p>
          <UButton type="button" size="sm" variant="outline" color="neutral" class="w-fit" @click="copyInvite(inviteResult.acceptUrl)">
            {{ t("workspace.invite.copyLink") }}
          </UButton>
          <p
            v-if="inviteCopyStatus"
            :role="inviteCopyStatus === 'failed' ? 'alert' : 'status'"
          >
            {{
              t(
                inviteCopyStatus === "copied"
                  ? "workspace.invite.copyLink.done"
                  : "workspace.invite.copyLink.failed",
              )
            }}
          </p>
        </div>
      </form>
    </div>
    <ConfirmDialog
      :open="removeTarget !== null"
      :title="
        removeTarget
          ? t('workspace.member.remove.title', { name: memberName(removeTarget) })
          : ''
      "
      :body="
        removeTarget
          ? t('workspace.member.remove.description', { name: memberName(removeTarget) })
          : ''
      "
      :action-label="t('workspace.member.remove.confirm')"
      :pending="removeMember.isPending.value"
      :error="removeError"
      @close="
        removeTarget = null;
        removeError = null;
      "
      @confirm="confirmRemove"
    />
  </details>
</template>
