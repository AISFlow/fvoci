<script setup lang="ts">
import { CODE_LANGUAGES, CODE_LINE_BACKGROUND, codeChromeHostKey, type TiptapEditor, useCodeBlockChrome } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { inject, onBeforeUnmount, watch } from "vue";

// The controls of the code block the caret is in (react/code-block-chrome.tsx):
// language, copy, line numbers, fold (long blocks) and wrap, and the line
// gutter with highlighted ({3-5}) and diff lines.
const props = defineProps<{ editor: TiptapEditor }>();
const { block, chrome, view, copyFailed, patch, setLanguage, copy } = useCodeBlockChrome(props.editor);
const host = inject(codeChromeHostKey, null);

function onLanguageChange(event: Event): void {
  setLanguage((event.target as HTMLSelectElement).value);
}

/** WHY: a chrome button's default mousedown would focus it and drop the
 * ProseMirror caret, so the next keys would not edit the block (wrap then
 * Enter to reach nine lines in workspace-wiki-vue-controls). Same as
 * react/menu-keyboard.ts preventSelectionLoss. The language select still
 * takes focus. */
function keepEditorFocus(event: MouseEvent): void {
  if (event.target instanceof HTMLElement && event.target.closest("button")) {
    event.preventDefault();
  }
}

function syncHost(): void {
  if (!host) return;
  if (!block.value) {
    host.wrap = null;
    host.folded = null;
    return;
  }
  host.wrap = chrome.value.wrap;
  host.folded = chrome.value.folded;
}

watch([block, chrome], syncHost, { immediate: true });
onBeforeUnmount(() => {
  if (!host) return;
  host.wrap = null;
  host.folded = null;
});
</script>

<template>
  <div
    v-if="block && view"
    class="fvoci-code-chrome"
    :data-linenos="chrome.linenos ? 'true' : 'false'"
    :data-wrap="chrome.wrap ? 'true' : 'false'"
    :data-folded="chrome.folded ? 'true' : 'false'"
    @mousedown="keepEditorFocus"
  >
    <div class="fvoci-format-cluster">
      <select
        class="fvoci-code-chrome__language"
        :aria-label="t('editor.code.language')"
        :value="view.language"
        :disabled="!block.editable"
        @change="onLanguageChange"
      >
        <option v-for="lang in CODE_LANGUAGES" :key="lang || 'plain'" :value="lang">{{ lang || "plain" }}</option>
      </select>
      <UButton type="button" size="xs" variant="outline" color="neutral" @click="copy">{{ t("editor.code.copy") }}</UButton>
    </div>
    <p v-if="copyFailed" role="alert">{{ t("editor.copy.failed") }}</p>
    <div class="fvoci-format-cluster">
      <UButton
        type="button"
        size="xs"
        variant="outline"
        color="neutral"
        :aria-pressed="chrome.linenos ? 'true' : 'false'"
        @click="patch({ linenos: !chrome.linenos })"
      >
        {{ t("editor.code.linenos") }}
      </UButton>
      <UButton
        v-if="view.foldable"
        type="button"
        size="xs"
        variant="outline"
        color="neutral"
        :aria-pressed="chrome.folded ? 'true' : 'false'"
        @click="patch({ folded: !chrome.folded })"
      >
        {{ t("editor.code.fold") }}
      </UButton>
      <UButton
        type="button"
        size="xs"
        variant="outline"
        color="neutral"
        :aria-pressed="chrome.wrap ? 'true' : 'false'"
        @click="patch({ wrap: !chrome.wrap })"
      >
        {{ t("editor.code.wrap") }}
      </UButton>
    </div>
    <pre v-if="view.lines" class="fvoci-code-linenos" aria-hidden="true"><span
        v-for="line in view.lines"
        :key="line.n"
        :data-hl="line.kind ?? undefined"
        :style="line.kind ? { background: CODE_LINE_BACKGROUND[line.kind] } : undefined"
      >{{ line.label }}{{ "\n" }}</span></pre>
  </div>
</template>
