<!--
  Sidebar navigation button for workspace/personal full-page screens (Today's
  reviews, Today's learning, My time). Uses `<button type="button">` so sidebar
  link invariants stay untouched.
-->
<script lang="ts">
  import type { Component } from 'svelte';

  interface Props {
    testId: string;
    active: boolean;
    label: string;
    onclick: () => void;
    icon: Component<{ class?: string; 'aria-hidden'?: 'true' | 'false' }>;
  }

  let { testId, active, label, onclick, icon: Icon }: Props = $props();
</script>

<button
  type="button"
  data-testid={testId}
  aria-current={active ? 'page' : undefined}
  class="flex min-h-8 w-full items-center gap-2 rounded-md px-2 text-left hover:bg-sidebar-accent aria-[current=page]:bg-sidebar-accent aria-[current=page]:font-medium roomy:min-h-touch"
  {onclick}
>
  <Icon class="size-5 shrink-0 p-0.5" aria-hidden="true" />
  <span class="min-w-0 flex-1 truncate">{label}</span>
</button>
