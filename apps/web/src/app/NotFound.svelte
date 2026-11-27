<!--
  What an address shows when there is nothing there for the reader: one that
  is not the app's, a workspace that is not there or not theirs, a board that
  is not there, not theirs, or of another workspace than the address says.
  All look the same, so that the page does not tell which (what the reader
  may not see is not said to exist), and each has the way home.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';

  import type { Router } from './router.svelte.ts';

  let { router }: { router: Router } = $props();

  function goHome(event: MouseEvent): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    router.go({});
  }
</script>

<div class="grid h-full place-items-center p-6 text-center" data-testid="not-found">
  <div>
    <p class="text-lg font-medium" role="alert">{m.app_page_not_found()}</p>
    <p class="mt-1 text-muted-foreground">{m.app_board_not_found_hint()}</p>
    <a
      class="mt-4 inline-block underline underline-offset-4 roomy:py-3"
      href={router.href({})}
      onclick={goHome}>{m['error.back-to-home']()}</a
    >
  </div>
</div>
