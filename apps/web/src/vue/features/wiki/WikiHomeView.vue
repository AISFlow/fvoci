<script setup lang="ts">
import { t, type I18nKey } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed } from "vue";
import { childrenByParent } from "@/features/workspace/wiki-tree";
import { trashPath } from "@/lib/href";
import type { TreeNode } from "@/lib/queries/documents";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import WikiBranch from "./WikiBranch.vue";
import "@/features/documents/document-shell.css";

const ROLE_LABEL: Record<string, I18nKey> = {
  guest: "role.guest",
  member: "role.member",
  admin: "role.admin",
  owner: "role.owner",
};

const props = defineProps<{
  slug: string;
  nodes: readonly TreeNode[];
  loading: boolean;
  error: string | null;
  creating: boolean;
  createError: string | null;
  canCreate: boolean;
  role: string;
  onRetry: () => void;
  onCreate: () => void;
}>();

const wikiRoots = computed(() =>
  props.nodes.filter((node) => node.parentId === null && node.projectId === null),
);
const byParent = computed(() => childrenByParent(props.nodes));
const empty = computed(() => !props.loading && !props.error && wikiRoots.value.length === 0);

function roleLabel(role: string): string {
  const key = ROLE_LABEL[role];
  return key ? t(key) : role;
}

function create(): void {
  if (!props.canCreate || props.creating) return;
  props.onCreate();
}
</script>

<template>
  <div class="wiki-home">
    <div class="wiki-home__head">
      <div class="wiki-home__intro">
        <h1 class="wiki-home__title">{{ t("nav.wiki") }}</h1>
        <p class="wiki-home__role">{{ t("wiki.role.current", { role: roleLabel(role) }) }}</p>
        <a class="wiki-home__trash-link underline underline-offset-2" :href="trashPath(slug)">{{
          t("trash.title")
        }}</a>
      </div>
      <UButton v-if="canCreate && !empty" type="button" :disabled="creating" @click="create">
        {{ creating ? t("doc.create.pending") : t("nav.newDocument") }}
      </UButton>
    </div>
    <p v-if="createError" role="alert" class="wiki-home__error">{{ createError }}</p>
    <div
      v-if="empty"
      class="mx-auto flex w-full max-w-lg flex-1 flex-col items-start justify-center gap-3 px-6 py-12"
    >
      <p class="break-keep text-lg font-semibold">{{ t("doc.empty") }}</p>
      <p v-if="canCreate" class="max-w-prose break-keep text-sm leading-relaxed text-muted">
        {{ t("doc.firstHint") }}
      </p>
      <UButton v-if="canCreate" type="button" size="sm" :disabled="creating" @click="create">
        {{ t("nav.newDocument") }}
      </UButton>
    </div>
    <section v-else class="wiki-home__section">
      <QueryLoading v-if="loading" />
      <QueryError v-else-if="error" :message="error" @retry="onRetry" />
      <p v-else-if="wikiRoots.length === 0" class="wiki-home__empty">{{ t("nav.wikiEmpty") }}</p>
      <ul v-else class="wiki-tree">
        <WikiBranch
          v-for="node in wikiRoots"
          :key="node.id"
          :slug="slug"
          :node="node"
          :by-parent="byParent"
        />
      </ul>
    </section>
  </div>
</template>
