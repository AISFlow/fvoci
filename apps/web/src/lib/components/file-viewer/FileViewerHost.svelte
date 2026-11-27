<!--
  The viewer of a file opened from a card's attachments or a body's file
  block (viewing.svelte.ts), over whatever screen it was opened on, a card's
  dialog included. Mounted once, in the app's frame; the viewer itself
  arrives when it is first needed (chunk.svelte.ts).

  While a version of the file is in the editor (FileEditor.svelte), the
  editor is shown in the viewer's place, holding the file's lease; the viewer
  comes back, made anew, when the editor is closed.
-->
<script lang="ts">
  import type { EditedFile } from '$lib/file-viewer/editing.svelte.ts';
  import type { EditLease } from '$lib/file-viewer/lease.svelte.ts';
  import type { Regional } from '$lib/logic/regional.ts';

  import { editorChunk, viewerChunk } from './chunk.svelte.ts';
  import { viewing } from './viewing.svelte.ts';

  interface Props {
    regional?: Regional | undefined;
  }

  let { regional }: Props = $props();

  const Viewer = $derived(viewerChunk.component);
  const Editor = $derived(editorChunk.component);

  // Gone with the screen it is on (the reader signed out): the file open is let go of, and
  // not shown again over the next.
  $effect(() => () => {
    viewing.forget();
  });
</script>

{#if Editor && viewing.edited && viewing.versions}
  <Editor
    file={viewing.edited}
    versions={viewing.versions}
    lease={viewing.lease}
    onClose={() => {
      viewing.endEdit();
    }}
  />
{:else if Viewer && viewing.file}
  <!-- The viewer is made for one file: another is another viewer. -->
  {#key viewing.file}
    <Viewer
      file={viewing.file}
      versions={viewing.versions}
      {regional}
      released={viewing.released}
      onClose={() => {
        viewing.close();
      }}
      onEdit={(edited: EditedFile, lease: EditLease) => {
        void viewing.edit(edited, lease);
      }}
      returnFocus={() => {
        viewing.returnFocus();
      }}
    />
  {/key}
{/if}
