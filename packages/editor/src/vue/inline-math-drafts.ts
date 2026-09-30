import type { Editor } from "@tiptap/core";
import {
  absolutePositionToRelativePosition,
  relativePositionToAbsolutePosition,
  ySyncPluginKey,
  type ProsemirrorBinding,
} from "@tiptap/y-tiptap";
import * as Y from "yjs";

export type InlineMathDraft = {
  value: string;
  focused: boolean;
  start: number | null;
  end: number | null;
  direction: "forward" | "backward" | "none" | null;
};

// #259: a remote update can replace the inline atom's Vue view. Keep only
// the transient field state outside that view, keyed by the Yjs item at the
// atom's start, rather than its shifting document position or LaTeX value.
// Nothing here writes the collaborative document or changes its format.
// y-tiptap publishes PluginKey<any>; these fields are owned by its sync plugin.
type SyncState = { doc: Y.Doc; type: Y.XmlFragment; binding: ProsemirrorBinding | null };

type Entry = { position: Y.RelativePosition; draft: InlineMathDraft };
const pools = new WeakMap<Editor, Map<string, Entry>>();

function identity(
  editor: Editor,
  pos: number,
): { key: string; position: Y.RelativePosition } | null {
  const sync = ySyncPluginKey.getState(editor.state) as SyncState | undefined;
  if (!sync?.binding || editor.state.doc.nodeAt(pos)?.type.name !== "mathInline") return null;
  const boundary = absolutePositionToRelativePosition(
    pos,
    sync.type,
    sync.binding.mapping,
  ) as Y.RelativePosition;
  const absolute = Y.createAbsolutePositionFromRelativePosition(boundary, sync.doc);
  if (!absolute) return null;
  // y-tiptap associates a text boundary with the character on its left.
  // Re-anchor the boundary to the atom on its right, so editing or deleting
  // the preceding text cannot change the draft's identity.
  let parent = absolute.type;
  let index = absolute.index;
  if (parent instanceof Y.XmlText) {
    if (index !== parent.length || !(parent.parent instanceof Y.XmlElement)) return null;
    const text = parent;
    const container = parent.parent;
    parent = container;
    index = container.toArray().indexOf(text) + 1;
  }
  if (!(parent instanceof Y.XmlElement)) return null;
  const atom = parent.get(index);
  if (!(atom instanceof Y.XmlElement) || atom.nodeName !== "mathInline") return null;
  const position = Y.createRelativePositionFromTypeIndex(parent, index);
  if (!position.item) return null;
  return { key: `${String(position.item.client)}:${String(position.item.clock)}`, position };
}

function pool(editor: Editor): Map<string, Entry> {
  const existing = pools.get(editor);
  if (existing) return existing;
  const entries = new Map<string, Entry>();
  pools.set(editor, entries);
  const prune = () => {
    const sync = ySyncPluginKey.getState(editor.state) as SyncState | undefined;
    if (!sync?.binding) return;
    for (const [key, entry] of entries) {
      const pos = relativePositionToAbsolutePosition(
        sync.doc,
        sync.type,
        entry.position,
        sync.binding.mapping,
      );
      // Deleted/replaced atoms must never hand their drafts to neighbours.
      if (pos === null || identity(editor, pos)?.key !== key) entries.delete(key);
    }
  };
  const revoke = () => {
    if (!editor.isEditable) entries.clear();
  };
  const destroy = () => {
    entries.clear();
    pools.delete(editor);
    editor.off("transaction", prune);
    editor.off("update", revoke);
    editor.off("destroy", destroy);
  };
  editor.on("transaction", prune);
  editor.on("update", revoke);
  editor.on("destroy", destroy);
  return entries;
}

export function inlineMathDrafts(editor: Editor, getPos: () => number | undefined) {
  const entries = pool(editor);
  const current = () => {
    const pos = getPos();
    return typeof pos === "number" ? identity(editor, pos) : null;
  };
  // A removed view's getPos is no longer valid during unmount. A reused
  // view can begin a later edit of another atom, with a fresh identity.
  let active = current();
  return {
    begin(): void {
      active = current();
    },
    isCurrent(): boolean {
      return active !== null && current()?.key === active.key;
    },
    read(): InlineMathDraft | undefined {
      return active ? entries.get(active.key)?.draft : undefined;
    },
    write(draft: InlineMathDraft): void {
      if (active && current()?.key === active.key) {
        entries.set(active.key, { position: active.position, draft });
      }
    },
    clear(): void {
      if (active) entries.delete(active.key);
    },
  };
}
