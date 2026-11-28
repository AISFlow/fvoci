<!--
  The tabs of the notifications list: every one of the inbox, the unread,
  the mentions, the cards given, and what was put away. The arrow keys go
  from tab to tab and show it (`arrowChoice`); Tab reaches the one shown. In
  a narrow pane they scroll sideways rather than wrap.
-->
<script lang="ts">
  import { arrowChoice } from '$lib/components/arrow-choice.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { InboxTab } from '$lib/state/notification-inbox.svelte.ts';

  interface Props {
    tab: InboxTab;
    onShow: (tab: InboxTab) => void;
  }

  let { tab, onShow }: Props = $props();

  const tabs = $derived<readonly { id: InboxTab; label: string }[]>([
    { id: 'all', label: m.notifications_tab_all() },
    { id: 'unread', label: m.notifications_tab_unread() },
    { id: 'mention', label: m.notifications_tab_mention() },
    { id: 'assignment', label: m.notifications_tab_assignment() },
    { id: 'archived', label: m.notifications_tab_archived() },
  ]);
</script>

<div
  role="tablist"
  aria-label={m.notifications_tabs()}
  data-testid="notification-tabs"
  class="flex min-w-0 shrink-0 items-center gap-1 overflow-x-auto border-b px-2 py-1"
  {@attach arrowChoice()}
>
  {#each tabs as one (one.id)}
    <button
      type="button"
      role="tab"
      aria-selected={one.id === tab}
      data-tab={one.id}
      class={[
        'inline-flex shrink-0 items-center rounded-md px-2.5 py-1 text-sm font-medium transition-colors roomy:min-h-touch',
        one.id === tab
          ? 'bg-accent text-accent-foreground'
          : 'text-muted-foreground hover:bg-muted hover:text-foreground',
      ]}
      onclick={() => {
        onShow(one.id);
      }}
    >
      {one.label}
    </button>
  {/each}
</div>
