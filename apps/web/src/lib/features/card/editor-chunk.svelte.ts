/**
 * The editor of a body, fetched when a card that can be edited is first
 * opened.
 *
 * The editor is the heaviest part of the app and most visits start on a
 * board, so it is not part of what the first screen loads: the build puts it
 * into a file of its own, and that file is asked for here. Until it has
 * arrived the card shows its body with the read-only view, which needs none
 * of it. Once fetched it stays for the rest of the visit.
 */
import type { Editor as TiptapEditor } from '@tiptap/core';

import { noteChunkFailure } from '$lib/chunk-reload.svelte.ts';
import type BodyEditor from '$lib/editor/BodyEditor.svelte';

class EditorChunk {
  /** The editor component, once it has arrived. */
  component = $state.raw<typeof BodyEditor | undefined>(undefined);
  /** Moves an editor's selection to the end synchronously (`Selection.atEnd`), once loaded. */
  selectEnd: ((editor: TiptapEditor) => void) | undefined;
  /** The last try to fetch it failed (the connection dropped, a new version was deployed). */
  failed = $state(false);
  #loading: Promise<void> | undefined;

  /** Asks for the editor unless it is here or on its way. Call it again to try again after a failure. */
  load(): void {
    if (this.component || this.#loading) return;
    this.failed = false;
    this.#loading = Promise.all([
      import('$lib/editor/BodyEditor.svelte'),
      import('$lib/editor/select-end.ts'),
    ])
      .then(([editor, { selectEnd }]) => {
        this.selectEnd = selectEnd;
        this.component = editor.default;
      })
      .catch(() => {
        this.failed = true;
        void noteChunkFailure();
      })
      .finally(() => {
        this.#loading = undefined;
      });
  }
}

export const editorChunk = new EditorChunk();
