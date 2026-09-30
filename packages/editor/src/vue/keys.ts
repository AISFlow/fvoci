import type { Component, InjectionKey } from "vue";
import type { AttachmentBlockBridge } from "../attachment-model.js";
import type { EntityResolver } from "../entities.js";

/* What the host gives the Vue node views. FvociEditor.vue provides them;
 * @tiptap/vue-3 renders each node view with EditorContent's app context and
 * provides (VueRenderer), so inject() reaches them there. */

/** Upload, download link and metadata of attachment blocks; null disables uploads. */
export const attachmentBridgeKey: InjectionKey<AttachmentBlockBridge | null> =
  Symbol("fvociAttachmentBridge");

/** Renders a URL embed (the web app's unfurl card); null shows the plain link card. */
export type UrlEmbedComponent = Component<{ url: string }>;
export const urlEmbedKey: InjectionKey<UrlEmbedComponent | null> = Symbol("fvociUrlEmbed");

/** Labels document, task and project embeds; null shows the stored reference. */
export const entityResolverKey: InjectionKey<EntityResolver | null> = Symbol("fvociEntityResolver");

/**
 * The code-block chrome's wrap/fold view options on the editor host
 * (`data-code-wrap` / `data-code-folded`). `null` means the caret is not
 * in a code block, so the attributes are omitted (react/code-block-chrome.tsx).
 */
export type CodeChromeHost = { wrap: boolean | null; folded: boolean | null };
export const codeChromeHostKey: InjectionKey<CodeChromeHost> = Symbol("fvociCodeChromeHost");
