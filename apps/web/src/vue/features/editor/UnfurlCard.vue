<script setup lang="ts">
import { SafeHtml } from "@fvoci/editor/vue";
import { asSafeHtml } from "@fvoci/editor/safe-html";
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import {
  isHttpUrl,
  isSandboxedIframeHtml,
  unfurlCardDataOf,
  unfurlDisplayTitle,
  type UnfurlOutput,
} from "@/features/workspace/unfurl";
import { unfurlQueryOptions } from "@/lib/queries/workspace";

// A URL embed's preview card (features/workspace/unfurl-card.tsx): the
// server's unfurl (Open Graph or GitHub) of an http(s) link.
const props = defineProps<{ workspaceId: string; url: string }>();
const allowed = computed(() => isHttpUrl(props.url));
const query = useQuery(() => ({
  ...unfurlQueryOptions(props.workspaceId, props.url),
  enabled: allowed.value,
}));

function parseUnfurl(data: unknown): UnfurlOutput | null {
  if (data === null || typeof data !== "object") return null;
  const value = data as Partial<UnfurlOutput>;
  if (value.kind !== "github_issue" && value.kind !== "github_pull" && value.kind !== "og") return null;
  if (typeof value.url !== "string") return null;
  return value as UnfurlOutput;
}

const parsed = computed(() => parseUnfurl(query.data.value));
const state = computed(() => {
  if (!allowed.value) return "failed" as const;
  if (query.isLoading.value) return "loading" as const;
  if (query.isError.value || !parsed.value) return "failed" as const;
  return "resolved" as const;
});
// Server-sanitized oEmbed markup: only a sandboxed iframe is shown as HTML.
const iframeHtml = computed(() => {
  const html = parsed.value?.html;
  return isSandboxedIframeHtml(html) ? asSafeHtml(html) : null;
});
const card = computed(() => (parsed.value ? unfurlCardDataOf(parsed.value) : null));
const title = computed(() => unfurlDisplayTitle(card.value?.title ?? "", props.url));
const imageUrl = computed(() => {
  const image = card.value?.imageUrl;
  return image && isHttpUrl(image) ? image : null;
});
const openLinkClass =
  "inline-flex h-8 items-center rounded-md border border-default bg-default px-3 text-sm font-medium hover:bg-elevated";
</script>

<template>
  <article v-if="state === 'loading'" class="w-full max-w-md rounded-lg border border-default py-4" data-entity="url">
    <p class="px-4" role="status">{{ t("unfurl.loading") }}</p>
  </article>
  <article v-else-if="state === 'failed'" class="w-full max-w-md rounded-lg border border-default py-4" data-entity="url">
    <p class="px-4 break-keep">{{ t("unfurl.failed") }}</p>
    <div class="px-4 pt-2">
      <a v-if="isHttpUrl(url)" :href="url" target="_blank" rel="noopener noreferrer" :class="openLinkClass">{{ t("unfurl.open") }}</a>
    </div>
  </article>
  <SafeHtml v-else-if="iframeHtml" class="fvoci-oembed" :html="iframeHtml" />
  <article v-else class="w-full max-w-md overflow-hidden rounded-lg border border-default" data-entity="url">
    <img v-if="imageUrl" :src="imageUrl" alt="" class="aspect-video w-full object-cover" />
    <div class="px-4 pt-4">
      <h2 class="font-medium break-keep">{{ title }}</h2>
      <p v-if="card && card.description !== ''" class="pt-1 text-sm text-muted break-keep">{{ card.description }}</p>
    </div>
    <div class="px-4 pt-2 pb-4">
      <a v-if="isHttpUrl(url)" :href="url" target="_blank" rel="noopener noreferrer" :class="openLinkClass">{{ t("unfurl.open") }}</a>
    </div>
  </article>
</template>
