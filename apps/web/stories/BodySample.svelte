<script lang="ts">
  import BodyView from '$lib/editor/BodyView.svelte';
  import { setFileSource } from '$lib/editor/files.ts';

  import { PEOPLE, SAMPLE_REFERENCES, sampleFileSource } from './body-samples.ts';

  let { doc }: { doc: unknown } = $props();

  // The stories have no server: pictures are made up from the file identifier, and the
  // records of the files of file blocks are the samples'.
  setFileSource(sampleFileSource);
</script>

<div class="max-w-xl rounded-lg border border-input-border">
  <BodyView
    {doc}
    resolveUser={(userId: string) => PEOPLE[userId]}
    references={SAMPLE_REFERENCES}
    taskLabel={(label: string, checked: boolean) => `${checked ? '끝낸 일' : '할 일'}: ${label}`}
    class="p-4"
  >
    {#snippet invalid(problems)}
      <div class="p-4 text-sm" role="alert">
        <p class="text-destructive">표시할 수 없는 본문입니다.</p>
        <ul class="mt-2 list-disc ps-5 font-mono text-xs text-muted-foreground">
          {#each problems as problem (problem)}
            <li>{problem}</li>
          {/each}
        </ul>
      </div>
    {/snippet}
    {#snippet empty()}
      <p class="p-4 text-sm text-muted-foreground">내용이 없습니다.</p>
    {/snippet}
  </BodyView>
</div>
