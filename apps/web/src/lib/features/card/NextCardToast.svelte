<!--
  Says that completing a card that repeats made its next card, to the one
  who completed it: "Made the next card #12 · Undo". Undo takes back the
  change that completed it (the card is not done and repeats again, and the
  next card goes) while it is the change to undo. It goes by itself after a
  while. Pressed over an open card, it leaves the card open (`CardDialog`).
-->
<script lang="ts">
  import XIcon from '@lucide/svelte/icons/x';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { nextCardNotices, type Workspace } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    workspace: Workspace;
  }

  let { workspace }: Props = $props();

  const notices = $derived(nextCardNotices(workspace));
  const notice = $derived(notices.notice);

  const text = $derived.by(() => {
    if (!notice) return '';
    if (notice.count > 1) return m.card_next_made_many({ count: notice.count });
    return notice.number === undefined
      ? m.card_next_made_unnumbered()
      : m.card_next_made({ number: notice.number });
  });
</script>

{#if notice}
  <div
    class="pointer-events-none fixed inset-x-0 bottom-[calc(var(--safe-bottom)+var(--keyboard)+3.5rem)] z-[60] flex justify-center px-3"
    role="status"
    aria-live="polite"
    data-testid="next-card-toast"
    data-next-card-toast
  >
    <div
      class="pointer-events-auto flex max-w-md items-center gap-1 rounded-lg border bg-popover py-1 pr-1 pl-3 text-sm text-popover-foreground shadow-md"
    >
      <p class="min-w-0 flex-1">{text}</p>
      <Button
        variant="ghost"
        class="shrink-0 roomy:min-h-touch"
        onclick={() => void notices.undo()}
      >
        {m.card_next_undo()}
      </Button>
      <button
        type="button"
        class="grid size-8 shrink-0 place-items-center rounded-md hover:bg-muted roomy:size-touch"
        aria-label={m.app_problem_dismiss()}
        onclick={() => {
          notices.dismiss(notice.id);
        }}
      >
        <XIcon class="size-4" aria-hidden="true" />
      </button>
    </div>
  </div>
{/if}
