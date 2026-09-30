<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { MemberOutput } from "@/lib/contracts";
import { membersQuery } from "@/lib/queries";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string; canManage: boolean }>();
const queryClient = useQueryClient();
const selectedGroupId = ref<string | null>(null);
const name = ref("");
const actionError = ref<string | null>(null);
const pending = ref(false);
const confirmDelete = ref(false);

const groupsQuery = useQuery(() => ({
  queryKey: ["workspaces", props.workspaceId, "groups"] as const,
  queryFn: async () =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/groups", {
        params: { path: { workspace_id: props.workspaceId } },
      }),
    ),
  retry: false as const,
}));
const members = useQuery(() => membersQuery(props.workspaceId));
const groupMembersQuery = useQuery(() => ({
  queryKey: ["workspaces", props.workspaceId, "groups", selectedGroupId.value, "members"] as const,
  enabled: Boolean(selectedGroupId.value),
  queryFn: async () => {
    const groupId = selectedGroupId.value;
    if (groupId === null) throw new Error("Group members query requires a selected group");
    return ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members", {
        params: { path: { workspace_id: props.workspaceId, group_id: groupId } },
      }),
    );
  },
  retry: false as const,
}));

const groups = computed(() => groupsQuery.data.value?.items ?? []);
const selected = computed(
  () => groups.value.find((group) => group.id === selectedGroupId.value) ?? null,
);
const memberItems = computed(() => members.data.value?.items ?? []);
const selectedMemberIds = computed(
  () => new Set((groupMembersQuery.data.value?.items ?? []).map((row) => row.userId)),
);
const candidates = computed(() =>
  memberItems.value.filter((member) => !selectedMemberIds.value.has(member.userId)),
);
const groupMemberRows = computed(() => groupMembersQuery.data.value?.items ?? []);

const groupsError = computed(() =>
  groupsQuery.error.value instanceof ProblemError
    ? groupsQuery.error.value.title
    : groupsQuery.error.value
      ? t("group.loadFailed")
      : null,
);
const membersError = computed(() =>
  groupMembersQuery.error.value instanceof ProblemError
    ? groupMembersQuery.error.value.title
    : groupMembersQuery.error.value
      ? t("group.membersLoadFailed")
      : null,
);
const candidatesError = computed(() =>
  members.error.value instanceof ProblemError
    ? members.error.value.title
    : members.error.value
      ? t("group.candidatesLoadFailed")
      : null,
);

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

function memberLabel(userId: string): string {
  const member = memberItems.value.find((item: MemberOutput) => item.userId === userId);
  return member ? `${formatPersonName(member)} (${member.email})` : t("group.memberUnavailable");
}

async function refreshGroups(): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: ["workspaces", props.workspaceId, "groups"] });
}

async function refreshMembers(): Promise<void> {
  await queryClient.invalidateQueries({
    queryKey: ["workspaces", props.workspaceId, "groups", selectedGroupId.value, "members"],
  });
}

const create = useMutation({
  mutationFn: async (groupName: string) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/groups", {
        params: { path: { workspace_id: props.workspaceId } },
        body: { name: groupName },
      }),
    ),
  onSuccess: async (created) => {
    actionError.value = null;
    name.value = "";
    selectedGroupId.value = created.id;
    await refreshGroups();
  },
  onError: (err) => {
    actionError.value = failMessage(err);
  },
});

async function run(action: () => Promise<void>): Promise<void> {
  pending.value = true;
  try {
    await action();
  } finally {
    pending.value = false;
  }
}

function onCreate(): void {
  const trimmed = name.value.trim();
  if (trimmed === "") {
    actionError.value = t("group.name.required");
    return;
  }
  create.mutate(trimmed);
}

function selectGroup(id: string): void {
  selectedGroupId.value = id;
  actionError.value = null;
  confirmDelete.value = false;
}

function addMember(event: Event): void {
  const form = event.currentTarget as HTMLFormElement;
  const value = new FormData(form).get("userId");
  const group = selected.value;
  if (typeof value !== "string" || value === "" || !group) return;
  void run(async () => {
    actionError.value = null;
    await ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members", {
        params: { path: { workspace_id: props.workspaceId, group_id: group.id } },
        body: { userId: value },
      }),
    );
    form.reset();
    await refreshMembers();
  }).catch((err: unknown) => {
    actionError.value = failMessage(err);
  });
}

function removeMember(userId: string): void {
  const group = selected.value;
  if (!group) return;
  void run(async () => {
    actionError.value = null;
    await ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/groups/{group_id}/members", {
        params: { path: { workspace_id: props.workspaceId, group_id: group.id } },
        body: { userId },
      }),
    );
    await refreshMembers();
  }).catch((err: unknown) => {
    actionError.value = failMessage(err);
  });
}

function deleteGroup(): void {
  const group = selected.value;
  if (!group) return;
  void run(async () => {
    actionError.value = null;
    await ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/groups/{group_id}", {
        params: { path: { workspace_id: props.workspaceId, group_id: group.id } },
      }),
    );
    selectedGroupId.value = null;
    confirmDelete.value = false;
    await refreshGroups();
  }).catch((err: unknown) => {
    actionError.value = failMessage(err);
  });
}
</script>

<template>
  <details class="settings-disclosure">
    <summary class="settings-disclosure__summary">{{ t("group.management.title") }}</summary>
    <div class="settings-disclosure__body">
      <p v-if="groupsQuery.isPending.value" role="status">{{ t("load.loading") }}</p>
      <p v-if="groupsError" role="alert" class="settings-notice settings-notice--danger">
        {{ groupsError }}
        <UButton
          type="button"
          size="sm"
          variant="outline"
          color="neutral"
          @click="groupsQuery.refetch()"
        >
          {{ t("load.retry") }}
        </UButton>
      </p>
      <p v-else-if="groups.length === 0 && !groupsQuery.isPending.value" class="text-muted">{{
        t("group.empty")
      }}</p>
      <ul v-else class="flex flex-col gap-1">
        <li v-for="group in groups" :key="group.id">
          <UButton
            type="button"
            size="sm"
            :variant="group.id === selectedGroupId ? 'solid' : 'outline'"
            :color="group.id === selectedGroupId ? 'primary' : 'neutral'"
            :aria-pressed="group.id === selectedGroupId"
            :disabled="pending"
            @click="selectGroup(group.id)"
          >
            {{ group.name }}
          </UButton>
        </li>
      </ul>
      <form v-if="canManage" class="settings-form mt-3" novalidate @submit.prevent="onCreate">
        <p class="font-medium">{{ t("group.add") }}</p>
        <div class="settings-form__row">
          <div>
            <label for="group-name">{{ t("group.name") }}</label>
            <UInput id="group-name" v-model="name" :disabled="pending || create.isPending.value" />
          </div>
          <UButton type="submit" size="sm" :disabled="pending || create.isPending.value">{{
            t("group.create")
          }}</UButton>
        </div>
      </form>
      <section v-if="selected" class="mt-4 flex flex-col gap-3">
        <h2 class="font-medium">{{ selected.name }}</h2>
        <p v-if="membersError" role="alert" class="settings-notice settings-notice--danger">
          {{ membersError }}
          <UButton
            type="button"
            size="sm"
            variant="outline"
            color="neutral"
            @click="groupMembersQuery.refetch()"
          >
            {{ t("load.retry") }}
          </UButton>
        </p>
        <p
          v-if="groupMemberRows.length === 0 && !groupMembersQuery.isPending.value"
          class="text-muted"
        >
          {{ t("group.emptyMembers") }}
        </p>
        <ul v-else class="flex flex-col divide-y">
          <li
            v-for="row in groupMemberRows"
            :key="row.userId"
            class="flex items-center justify-between gap-2 py-2"
          >
            <span>{{ memberLabel(row.userId) }}</span>
            <UButton
              v-if="canManage"
              type="button"
              size="sm"
              variant="outline"
              color="neutral"
              :disabled="pending"
              @click="removeMember(row.userId)"
            >
              {{ t("group.removeMember") }}
            </UButton>
          </li>
        </ul>
        <p
          v-if="canManage && candidatesError"
          role="alert"
          class="settings-notice settings-notice--danger"
        >
          {{ candidatesError }}
          <UButton
            type="button"
            size="sm"
            variant="outline"
            color="neutral"
            @click="members.refetch()"
          >
            {{ t("load.retry") }}
          </UButton>
        </p>
        <form v-else-if="canManage" class="settings-form__row" @submit.prevent="addMember">
          <div>
            <label for="group-member">{{ t("group.members") }}</label>
            <select
              id="group-member"
              name="userId"
              class="h-11 min-w-56 rounded-md border border-default bg-default px-3"
              :disabled="pending || candidates.length === 0"
            >
              <option value="">{{ t("group.members") }}</option>
              <option v-for="member in candidates" :key="member.userId" :value="member.userId">
                {{ formatPersonName(member) }} ({{ member.email }})
              </option>
            </select>
          </div>
          <UButton type="submit" size="sm" :disabled="pending || candidates.length === 0">{{
            t("group.addMember")
          }}</UButton>
        </form>
        <div v-if="canManage && confirmDelete" class="flex flex-col gap-2">
          <p>{{ t("group.delete.confirm.body", { name: selected.name }) }}</p>
          <div class="flex gap-2">
            <UButton
              type="button"
              size="sm"
              variant="outline"
              color="neutral"
              @click="confirmDelete = false"
            >
              {{ t("common.dismiss") }}
            </UButton>
            <UButton type="button" size="sm" :disabled="pending" @click="deleteGroup">{{
              t("group.delete")
            }}</UButton>
          </div>
        </div>
        <UButton
          v-else-if="canManage"
          type="button"
          size="sm"
          variant="outline"
          color="neutral"
          @click="confirmDelete = true"
        >
          {{ t("group.delete") }}
        </UButton>
      </section>
      <p v-if="actionError" role="alert" class="settings-notice settings-notice--danger">{{
        actionError
      }}</p>
    </div>
  </details>
</template>
