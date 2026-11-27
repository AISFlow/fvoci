/**
 * The file viewer, fetched the first time a file is opened, and the editor
 * of Hangul documents, fetched the first time one is edited.
 *
 * Most visits view no file, so the viewer is not part of what the first
 * screen loads; the renderers and their libraries are further away still, in
 * the frame's own files (src/viewer-frame/), fetched by the frame. The editor
 * is further away again: its component arrives with the first "Edit", and the
 * editor itself (rhwp-studio and rhwp's engine) with its frame's files
 * (src/editor-frame/). Once here each stays for the rest of the visit.
 */
import type { Component } from 'svelte';

import { noteChunkFailure } from '$lib/chunk-reload.svelte.ts';

import type FileEditor from './FileEditor.svelte';
import type FileViewer from './FileViewer.svelte';

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- any component, whatever its props.
class Chunk<T extends Component<any>> {
  /** The component, once it has arrived. */
  component = $state.raw<T | undefined>(undefined);
  readonly #fetch: () => Promise<{ default: T }>;
  #loading: Promise<boolean> | undefined;

  constructor(fetch: () => Promise<{ default: T }>) {
    this.#fetch = fetch;
  }

  /** Asks for the component unless it is here or on its way; says whether it arrived. */
  load(): Promise<boolean> {
    if (this.component) return Promise.resolve(true);
    this.#loading ??= this.#fetch()
      .then((module) => {
        this.component = module.default;
        return true;
      })
      .catch(() => {
        void noteChunkFailure();
        return false;
      })
      .finally(() => {
        this.#loading = undefined;
      });
    return this.#loading;
  }
}

export const viewerChunk = new Chunk<typeof FileViewer>(() => import('./FileViewer.svelte'));
export const editorChunk = new Chunk<typeof FileEditor>(() => import('./FileEditor.svelte'));
