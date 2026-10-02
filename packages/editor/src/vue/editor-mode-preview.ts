import type { Editor } from "@tiptap/core";
import { DOMSerializer, type DOMOutputSpec, type Node as PmNode } from "@tiptap/pm/model";
import { t } from "@fvoci/i18n";
import {
  decodeFilename,
  isStoredAttachmentId,
  type AttachmentBlockBridge,
} from "../attachment-model.js";
import { EMBED_KIND_KEY, type EmbedCardState } from "../embed-model.js";
import { isEmbedEntity, type EntityResolver } from "../entities.js";
import { asSafeHtml, type SafeHtml } from "../safe-html.js";
import { sanitizeRenderedHtml } from "../sanitize.js";

type PreviewOptions = {
  attachmentBridge?: AttachmentBlockBridge | null;
  entityResolver?: EntityResolver | null;
};
export type EditorModePreview = Readonly<{ html: SafeHtml }>;
type PreviewStyle = Readonly<{
  marker: string;
  tag: string;
  declarations: readonly (readonly [string, string])[];
}>;
// This attribute is absent from the sanitizer allowlist, so stored/user markup
// cannot supply a marker. Generated markers belong only to this render object.
const STYLE_MARKER = "data-fvoci-preview-css";
const previewStyles = new WeakMap<EditorModePreview, readonly PreviewStyle[]>();
const attr = (node: PmNode, key: string): string =>
  typeof node.attrs[key] === "string" ? node.attrs[key] : "";

/** Same card contract as the existing read-only AttachmentBlock: downloads use
 * the host bridge, filenames are decoded, and a missing bridge never invents a
 * URL. DOMSerializer escapes literal names/captions before sanitization. */
export function attachmentPreviewSpec(
  node: PmNode,
  bridge?: AttachmentBlockBridge | null,
): DOMOutputSpec {
  const id = attr(node, "id");
  const name = decodeFilename(attr(node, "name")) || t("editor.block.attachment");
  const stored = isStoredAttachmentId(id);
  const label: DOMOutputSpec = [
    "span",
    { class: "afn-attachment-name" },
    stored ? name : t("editor.attach.unselected"),
  ];
  const icon: DOMOutputSpec = [
    "span",
    { class: "afn-attachment-icon", "aria-hidden": "true" },
    node.attrs.image === true ? "🖼️" : "📎",
  ];
  const card: DOMOutputSpec =
    stored && bridge
      ? ["a", { class: "afn-attachment", "data-id": id, href: bridge.downloadUrl(id) }, icon, label]
      : ["div", { class: "afn-attachment", "data-id": id }, icon, label];
  const caption = attr(node, "caption");
  return [
    "figure",
    { "data-align": attr(node, "align") },
    card,
    ...(caption ? [["figcaption", {}, caption] satisfies DOMOutputSpec] : []),
  ];
}

/** Existing EmbedCard presentation states, retaining the actual entity/ref
 * tuple even when labels are inaccessible. A resolved label never replaces
 * the stored reference or creates a new target. */
export function embedPreviewSpec(node: PmNode, state?: EmbedCardState): DOMOutputSpec {
  const entity = attr(node, "entity");
  const ref = attr(node, "ref");
  const kind = isEmbedEntity(entity) ? t(EMBED_KIND_KEY[entity]) : entity;
  const label =
    state?.state === "resolved"
      ? state.snapshot.label
      : state?.state === "inaccessible"
        ? t("editor.embed.inaccessible", { kind })
        : ref || t("editor.embed.noRef");
  return [
    "div",
    { class: "afn-embed", "data-entity": entity, "data-id": attr(node, "id") },
    ["span", { class: "afn-embed-kind" }, kind],
    ["span", { class: "afn-embed-ref" }, label],
    ...(label !== ref && ref
      ? [["span", { class: "afn-embed-target" }, ref] satisfies DOMOutputSpec]
      : []),
    ...(state?.state === "resolved" && state.snapshot.status
      ? [["span", { class: "afn-embed-meta" }, state.snapshot.status] satisfies DOMOutputSpec]
      : []),
  ];
}

/** Snapshot the live immutable PM document, retain its existing schema/marks,
 * and use the host's existing resolver for supported reference cards. Only
 * these schema atom serializers need presentation: their getHTML specs are
 * deliberately empty because rich mode renders them with Vue node views. */
export async function editorModePreview(
  editor: Editor,
  options: PreviewOptions = {},
  signal?: AbortSignal,
): Promise<EditorModePreview> {
  signal?.throwIfAborted();
  const doc = editor.state.doc;
  const schema = editor.schema;
  const document = editor.view.dom.ownerDocument;
  const references = new Map<PmNode, Promise<EmbedCardState>>();
  doc.descendants((node) => {
    if (node.type.name !== "embed" || !options.entityResolver) return;
    const entity = attr(node, "entity");
    const ref = attr(node, "ref");
    if (!isEmbedEntity(entity) || entity === "url" || !ref) return;
    references.set(
      node,
      options.entityResolver(entity, ref).then(
        (snapshot): EmbedCardState =>
          snapshot ? { state: "resolved", snapshot } : { state: "inaccessible" },
        (): EmbedCardState => ({ state: "inaccessible" }),
      ),
    );
  });
  const pending = Promise.all(
    [...references].map(async ([node, pending]) => [node, await pending] as const),
  );
  // Host metadata requests may be shared with rich node views. Retire only this
  // producer's wait immediately, leaving that IO under its existing host owner.
  const pairs = signal
    ? await new Promise<Awaited<typeof pending>>((resolve, reject) => {
        const onAbort = () => {
          signal.removeEventListener("abort", onAbort);
          reject(
            signal.reason instanceof Error
              ? signal.reason
              : new DOMException("Preview retired", "AbortError"),
          );
        };
        signal.addEventListener("abort", onAbort, { once: true });
        void pending.then(
          (value) => {
            signal.removeEventListener("abort", onAbort);
            resolve(value);
          },
          (error: unknown) => {
            signal.removeEventListener("abort", onAbort);
            reject(
              error instanceof Error
                ? error
                : new Error("Preview metadata resolution failed", { cause: error }),
            );
          },
        );
        if (signal.aborted) onAbort();
      })
    : await pending;
  const resolved = new Map(pairs);
  signal?.throwIfAborted();
  const shared = DOMSerializer.fromSchema(schema);
  const serializer = new DOMSerializer(
    {
      ...shared.nodes,
      attachment: (node) => attachmentPreviewSpec(node, options.attachmentBridge),
      embed: (node) => embedPreviewSpec(node, resolved.get(node)),
    },
    shared.marks,
  );
  const host = document.createElement("div");
  host.appendChild(serializer.serializeFragment(doc.content, { document }));
  return sanitizeEditorModePreview(host.innerHTML);
}

export function sanitizeEditorModePreview(html: string): EditorModePreview {
  const styles: PreviewStyle[] = [];
  // sanitize-html emits lowercase tags and double-quoted, escaped attributes;
  // its escapeHtml escapes <, > and attribute quotes. Work only on that canonical
  // output, never raw markup or a DOM parsed with CSP-blocked style attributes.
  const sanitized = sanitizeRenderedHtml(html);
  const withoutStyles = sanitized.replace(/<[a-z][a-z0-9]*\b[^>]*>/g, (opening) => {
    const style = /\sstyle="([^"]*)"/.exec(opening);
    if (!style?.[1]) return opening;
    const tag = /^<([a-z][a-z0-9]*)\b/.exec(opening)?.[1];
    if (!tag) throw new Error("Invalid sanitized preview tag");
    const marker = String(styles.length);
    // The existing sanitizer has already parsed and bounded these declarations.
    // Its allowed values contain neither semicolons nor embedded colons/entities.
    const declarations: [string, string][] = style[1].split(";").map((declaration) => {
      const colon = declaration.indexOf(":");
      if (colon < 1) throw new Error("Invalid sanitized preview style");
      return [declaration.slice(0, colon).trim(), declaration.slice(colon + 1).trim()];
    });
    styles.push({ marker, tag, declarations });
    return opening.replace(style[0], ` ${STYLE_MARKER}="${marker}"`);
  });
  const result = { html: asSafeHtml(withoutStyles) };
  previewStyles.set(result, styles);
  return result;
}

/** CSSOM is the existing permitted rendering boundary (as for editor carets).
 * Only this producer's already-sanitized sidecar reaches it; the native SafeHtml
 * sink receives no style attribute. No CSS or URI is recovered from the DOM. */
export function applyEditorModePreviewStyles(root: HTMLElement, preview: EditorModePreview): void {
  const styles = previewStyles.get(preview);
  if (!styles) throw new Error("Unowned preview presentation");
  for (const entry of styles) {
    const element = root.querySelector<HTMLElement>(`[${STYLE_MARKER}="${entry.marker}"]`);
    if (!element || element.tagName.toLowerCase() !== entry.tag) continue;
    for (const [property, value] of entry.declarations) element.style.setProperty(property, value);
    element.removeAttribute(STYLE_MARKER);
  }
}
