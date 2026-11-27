<script lang="ts">
  import { Checkbox } from '$lib/ui/checkbox/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { Switch } from '$lib/ui/switch/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  const surfaces = ['bg-background', 'bg-muted'] as const;
</script>

<div class="grid gap-6 md:grid-cols-2">
  {#each surfaces as surface (surface)}
    <section class={['grid gap-4 rounded-lg p-4', surface]}>
      <div class="grid gap-1.5">
        <Label for="title-{surface}">제목 Title</Label>
        <Input id="title-{surface}" placeholder="보드 이름" />
      </div>
      <div class="grid gap-1.5">
        <Label for="note-{surface}">설명 Description</Label>
        <Textarea
          id="note-{surface}"
          placeholder="두 줄 이상의 한글 설명을 적어 줄 높이를 봅니다."
        />
      </div>
      <div class="flex items-center gap-2">
        <Checkbox id="done-{surface}" />
        <Label for="done-{surface}">완료 Done</Label>
      </div>
      <div class="flex items-center gap-2">
        <Switch id="share-{surface}" />
        <Label for="share-{surface}">공유 Share</Label>
      </div>
    </section>
  {/each}
</div>
