<!--
  The step of the sign-in page for somebody the organization's new provider does
  not know yet, while the organization moves from another one: the server holds
  their sign-in for this browser, and they say whether they had an account. If
  they had, the browser goes to the provider used before and comes back in that
  account, with the new sign-in connected to it. If not, the held sign-in ends
  in a new account. A refusal is told in place; the reason of one that came
  back from the old provider is the sign-in page's to tell (`oidc_error`).
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import { oidcProblemOf } from './oidc-problems.ts';
  import { problemOf } from './problems.ts';

  interface Props {
    /** The organization's name, once the server has said it; empty if it has none. */
    name: Promise<string>;
    /** Sends the browser to the provider used before; a refusal rejects. */
    onCarryOver: () => Promise<void>;
    /** Ends the held sign-in in a new account and goes on signed in; a refusal rejects. */
    onStartAnew: () => Promise<void>;
    /** Back to the sign-in form. */
    onCancel: () => void;
  }

  let { name, onCarryOver, onStartAnew, onCancel }: Props = $props();
  const uid = $props.id();

  let working = $state(false);
  let problem = $state<string | undefined>(undefined);

  async function go(doing: () => Promise<void>): Promise<void> {
    if (working) return;
    working = true;
    problem = undefined;
    try {
      // Done, the browser is leaving this page: it stays busy, so that a second
      // press starts nothing in place of this.
      await doing();
    } catch (error) {
      problem =
        isBackendError(error) && error.reason?.startsWith('oidc_')
          ? oidcProblemOf(error.reason.replaceAll('_', '-'))
          : problemOf(error);
      working = false;
    }
  }

  /** Back from the provider by the browser's back button, the page as it was left: no longer busy. */
  function onpageshow(event: PageTransitionEvent): void {
    if (event.persisted) working = false;
  }
</script>

<svelte:window {onpageshow} />

<main class="grid min-h-dvh place-items-center p-4">
  <section
    class="grid w-full max-w-sm gap-4"
    aria-labelledby="{uid}-title"
    data-testid="sign-in-choice"
  >
    <div class="text-center">
      <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      <h1 id="{uid}-title" class="mt-2 text-lg font-medium">{m.account_held_title()}</h1>
    </div>
    <!-- Shown at once; the organization is named when the server has said its name. -->
    <p class="text-sm text-muted-foreground">
      {#await name}
        {m.account_held_info_unnamed()}
      {:then said}
        {said ? m.account_held_info({ name: said }) : m.account_held_info_unnamed()}
      {:catch}
        {m.account_held_info_unnamed()}
      {/await}
    </p>

    <div class="grid gap-1.5">
      <Button
        class="h-auto min-h-9 py-2 whitespace-normal roomy:min-h-touch"
        disabled={working}
        data-testid="held-carry-over"
        onclick={() => void go(onCarryOver)}>{m.account_held_carry_over()}</Button
      >
      <p class="text-xs text-muted-foreground">{m.account_held_carry_over_hint()}</p>
    </div>
    <div class="grid gap-1.5">
      <Button
        variant="outline"
        class="roomy:min-h-touch"
        disabled={working}
        data-testid="held-start-anew"
        onclick={() => void go(onStartAnew)}>{m.account_held_start_anew()}</Button
      >
      <p class="text-xs text-muted-foreground">{m.account_held_start_anew_hint()}</p>
    </div>

    {#if problem}
      <p class="text-sm text-destructive" role="alert">{problem}</p>
    {/if}

    <Button variant="link" class="roomy:min-h-touch" disabled={working} onclick={onCancel}
      >{m.account_held_cancel()}</Button
    >
  </section>
</main>
