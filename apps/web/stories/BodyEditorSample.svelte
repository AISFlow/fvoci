<script lang="ts">
  import BodyEditor from '$lib/editor/BodyEditor.svelte';
  import BodyView from '$lib/editor/BodyView.svelte';
  import { setFileSource } from '$lib/editor/files.ts';
  import type { DocNode } from '$lib/editor/schema.ts';

  import { PEOPLE, SAMPLE_REFERENCES, sampleFileSource } from './body-samples.ts';

  let { doc, editable = true }: { doc: unknown; editable?: boolean } = $props();

  // The stories have no server: pictures are made up from the file identifier, and the
  // records of the files of file blocks are the samples'.
  setFileSource(sampleFileSource);
  const users = Object.entries(PEOPLE).map(([id, name]) => ({ id, name }));

  /** What the editor would store; `undefined` until it says, or when it refused the document. */
  let stored = $state.raw<DocNode | null | undefined>(undefined);
  let uploads = 0;
</script>

<div class="grid max-w-5xl grid-cols-1 gap-4 lg:grid-cols-2">
  <div class="rounded-lg border border-input-border">
    <BodyEditor
      doc={doc as DocNode | null}
      {editable}
      {users}
      resolveUser={(userId: string) => PEOPLE[userId]}
      references={SAMPLE_REFERENCES}
      uploadFile={() => Promise.resolve(`file-${(uploads += 1)}`)}
      placeholder="내용을 입력하세요."
      class="min-h-40 p-4"
      onChange={(next: DocNode | null) => {
        stored = next;
      }}
      onLoad={(loaded: DocNode | null | undefined) => {
        stored = loaded;
      }}
    />
  </div>
  <div class="rounded-lg border border-input-border">
    <BodyView
      doc={stored ?? null}
      resolveUser={(userId: string) => PEOPLE[userId]}
      references={SAMPLE_REFERENCES}
      class="p-4"
    >
      {#snippet empty()}
        <p class="p-4 text-sm text-muted-foreground">내용이 없습니다.</p>
      {/snippet}
    </BodyView>
  </div>
</div>
