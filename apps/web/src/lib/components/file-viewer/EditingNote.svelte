<!--
  The line under the viewer's header (FileViewer.svelte) that says who has
  the file open in the editor (lib/file-viewer/lease.svelte.ts): the reader
  in another window, somebody named, or somebody else where the reader may
  not know who. Else, what came of the last "Edit": the editor could not be
  opened, or a newer version was saved meanwhile and is shown instead.

  Always in the page, empty and out of sight when it says nothing, so that
  what it comes to say is announced.
-->
<script lang="ts">
  import type { LeaseWatch } from '$lib/file-viewer/lease.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';

  interface Props {
    /** The line's identifier, for "Edit" to be described by. */
    id?: string | undefined;
    /** Who edits the file, where it is asked. */
    watch: LeaseWatch | undefined;
    /** What came of the last "Edit", if it is to be said. */
    note: 'failed' | 'newer' | undefined;
    /** The name to show for a person; undefined for someone unknown. */
    userName: (userId: string) => string | undefined;
  }

  let { id, watch, note, userName }: Props = $props();

  const said = $derived.by((): string => {
    const editor = watch?.editor;
    if (editor?.who === 'me') return m.viewer_editing_me();
    if (editor?.who === 'named') {
      return m.viewer_editing_named({
        name: userName(editor.userId) ?? m['centerPanel.unknown-user'](),
      });
    }
    if (editor) return m.viewer_editing_someone();
    if (note === 'newer') return m.viewer_edit_newer();
    if (note === 'failed') return m.viewer_edit_failed();
    return '';
  });
</script>

<p
  {id}
  class={said ? 'border-b px-4 py-1 text-sm text-muted-foreground' : 'sr-only'}
  role="status"
  data-testid="file-viewer-editing"
  data-editing={watch?.editing === true}
>
  {said}
</p>
