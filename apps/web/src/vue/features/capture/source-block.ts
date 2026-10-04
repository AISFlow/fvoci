import type { TiptapEditor } from "@fvoci/editor/vue";
import { extractText } from "@fvoci/editor/extract";
/** Read the existing block UUID. No node schema/CRDT copy or alternate codec. */
export function sourceBlockSelection(
  editor: TiptapEditor | null,
): { anchor: string; title: string } | null {
  if (!editor || editor.isDestroyed) return null;
  const { $from } = editor.state.selection;
  for (let depth = $from.depth; depth > 0; depth--) {
    const node = $from.node(depth);
    const id: unknown = node.attrs.id;
    const title = extractText(node.toJSON()).trim();
    if (typeof id === "string" && id && title)
      return { anchor: id, title: Array.from(title).slice(0, 300).join("") };
  }
  return null;
}
export function originHref(
  slug: string,
  displayId: string,
  anchor: string | null | undefined,
): string {
  return `/w/${encodeURIComponent(slug.toLowerCase())}/${encodeURIComponent(displayId)}${anchor ? `#block=${encodeURIComponent(anchor)}` : ""}`;
}
