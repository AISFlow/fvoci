<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";
import { computed, inject, nextTick, ref, shallowRef, useTemplateRef, watch } from "vue";
import { EMBED_KIND_KEY, type EmbedCardState, resolveEmbedProps } from "../embed-model.js";
import { EMBED_ENTITIES, type EmbedEntity, isEmbedEntity } from "../entities.js";
import EmbedCard from "./EmbedCard.vue";
import { entityResolverKey, urlEmbedKey } from "./keys.js";
import { useEditable } from "./use-editable.js";

// The embed block (react/blocks.tsx EmbedBlockView): a document, task,
// project or URL card; editable embeds open a kind + reference form.
const props = defineProps(nodeViewProps);
const editable = useEditable(props.editor);
const resolver = inject(entityResolverKey, null);
const urlEmbed = inject(urlEmbedKey, null);

const entity = computed<EmbedEntity>(() => {
  const raw = String(props.node.attrs.entity ?? "document");
  return isEmbedEntity(raw) ? raw : "document";
});
const refValue = computed(() => String(props.node.attrs.ref ?? ""));

const editing = ref(false);
/** WHY: #644 리뷰 #3 — 빈 ref 자동 열기는 갓 삽입한 블록을 위한 것이다. 마운트 시점에 고정해,
 * 권한이 돌아올 때 문서 안 빈 embed 가 전부 열려 캐럿을 뺏지 않게 한다. */
const autoOpen = editable.value && refValue.value.trim() === "";
const showEditor = computed(
  () => editable.value && (editing.value || (autoOpen && refValue.value.trim() === "")),
);
/** The form's values when they differ from the node's; null otherwise. */
let draft: { entity: EmbedEntity; ref: string } | null = null;
const form = useTemplateRef<HTMLElement>("form");
const kindSelect = useTemplateRef<HTMLSelectElement>("kindSelect");
const refInput = useTemplateRef<HTMLTextAreaElement>("refInput");

function readForm(container: HTMLElement): { entity: EmbedEntity; ref: string } {
  const input = container.querySelector("textarea");
  const select = container.querySelector("select");
  const raw = input instanceof HTMLTextAreaElement ? input.value : "";
  const selected =
    select instanceof HTMLSelectElement && isEmbedEntity(select.value) ? select.value : "document";
  return resolveEmbedProps(raw, selected);
}

function commit(next: { entity: EmbedEntity; ref: string }): void {
  props.updateAttributes(next);
}

function rememberDraft(): void {
  if (!form.value) return;
  const next = readForm(form.value);
  /* WHY: rev-687 F2 — 원래 값으로 돌아온 초안은 커밋할 것이 없다(빈 undo 스텝·빈 협업 업데이트). */
  draft = next.entity === entity.value && next.ref === refValue.value ? null : next;
}

function commitIfLeaving(event: FocusEvent): void {
  const container = form.value;
  if (!container) return;
  if (event.relatedTarget instanceof Node && container.contains(event.relatedTarget)) return;
  draft = null;
  commit(readForm(container));
  editing.value = false;
}

function open(): void {
  editing.value = true;
}

// The form's fields are uncontrolled, like the React view's defaultValue:
// they get the node's values once, when the form opens (on mount for a new
// empty embed), and are never bound. Vue re-applies a bound value whenever
// the template re-renders, and the form re-renders on a peer's change to
// this node (data-entity), which would reset what was typed.
watch(
  showEditor,
  async (shown) => {
    if (!shown) return;
    const start = { entity: entity.value, ref: refValue.value };
    await nextTick();
    if (kindSelect.value) kindSelect.value.value = start.entity;
    if (!refInput.value) return;
    refInput.value.value = start.ref;
    refInput.value.focus();
  },
  { immediate: true },
);

// WHY: #644 리뷰 #2 — 권한이 사라지면 편집 상태도 닫고 마지막 초안을 커밋한다.
watch(editable, (value) => {
  if (value) return;
  editing.value = false;
  const pending = draft;
  draft = null;
  if (pending !== null) commit(pending);
});

// Document, task and project cards: labelled by the host's resolver when it has one.
const card = shallowRef<EmbedCardState>({ state: "plain", ref: "" });
watch(
  [entity, refValue],
  ([kind, value], _previous, onCleanup) => {
    if (kind === "url") return;
    if (!resolver || !value) {
      card.value = { state: "plain", ref: value };
      return;
    }
    card.value = { state: "loading" };
    let cancelled = false;
    onCleanup(() => {
      cancelled = true;
    });
    resolver(kind, value).then(
      (snapshot) => {
        if (!cancelled)
          card.value = snapshot ? { state: "resolved", snapshot } : { state: "inaccessible" };
      },
      () => {
        if (!cancelled) card.value = { state: "inaccessible" };
      },
    );
  },
  { immediate: true },
);
</script>

<template>
  <NodeViewWrapper>
    <div v-if="showEditor" ref="form" class="afn-embed afn-embed-edit" :data-entity="entity">
      <select
        ref="kindSelect"
        class="afn-embed-entity"
        :aria-label="t('editor.embed.kind')"
        @change="rememberDraft"
        @blur="commitIfLeaving"
      >
        <option v-for="item in EMBED_ENTITIES" :key="item" :value="item">{{
          t(EMBED_KIND_KEY[item])
        }}</option>
      </select>
      <textarea
        ref="refInput"
        class="afn-embed-ref-input"
        :aria-label="t('editor.embed.ref')"
        :placeholder="t('editor.embed.placeholder')"
        @input="rememberDraft"
        @blur="commitIfLeaving"
      />
    </div>
    <button
      v-else-if="editable"
      type="button"
      class="afn-embed-host"
      :aria-label="t('editor.embed.edit')"
      @mousedown.prevent
      @click="open"
    >
      <template v-if="entity === 'url'">
        <component :is="urlEmbed" v-if="refValue && urlEmbed" :url="refValue" />
        <EmbedCard v-else entity="url" :state="{ state: 'plain', ref: refValue }" />
      </template>
      <EmbedCard v-else :entity="entity" :state="card" />
    </button>
    <template v-else-if="entity === 'url'">
      <component :is="urlEmbed" v-if="refValue && urlEmbed" :url="refValue" />
      <EmbedCard v-else entity="url" :state="{ state: 'plain', ref: refValue }" />
    </template>
    <EmbedCard v-else :entity="entity" :state="card" />
  </NodeViewWrapper>
</template>
