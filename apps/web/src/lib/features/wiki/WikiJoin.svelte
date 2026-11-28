<!--
  The bar above an open wiki or open board the reader is no member of: they read
  it as it is, and join it here, with the role the container gives whoever joins
  it (`minimumRole`).
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { MemberRole } from '$lib/logic/wire.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    joining: boolean;
    /** The role joining gives: the container's `minimumRole`. */
    role: MemberRole | undefined;
    kind?: 'wiki' | 'board';
    /** Joins; the container is then read again, for the role joined with. */
    onJoin: () => void;
  }

  let { joining, role, kind = 'wiki', onJoin }: Props = $props();

  const uid = $props.id();
  const ROLE_LABELS = {
    commenter: () => m['BoardMember.roleCommenter'](),
    viewer: () => m['BoardMember.roleViewer'](),
  };
  const hint = $derived.by(() => {
    if (kind === 'board') {
      return role === 'commenter' || role === 'viewer'
        ? m.board_join_hint_as({ role: ROLE_LABELS[role]() })
        : m.board_join_hint();
    }
    return role === 'commenter' || role === 'viewer'
      ? m.wiki_join_hint_as({ role: ROLE_LABELS[role]() })
      : m.wiki_join_hint();
  });
  const buttonLabel = $derived(kind === 'board' ? m.board_join() : m.wiki_join());
  const testId = $derived(kind === 'board' ? 'board-join' : 'wiki-join');
</script>

<div
  class="flex shrink-0 flex-wrap items-center gap-x-3 gap-y-2 border-b bg-muted px-3 py-2"
  data-testid={testId}
>
  <p id="{uid}-hint" class="min-w-0 flex-1 text-sm text-muted-foreground">{hint}</p>
  <Button
    size="sm"
    class="roomy:min-h-touch"
    aria-describedby="{uid}-hint"
    aria-disabled={joining}
    aria-busy={joining}
    onclick={() => {
      if (!joining) onJoin();
    }}
  >
    {buttonLabel}
  </Button>
</div>
