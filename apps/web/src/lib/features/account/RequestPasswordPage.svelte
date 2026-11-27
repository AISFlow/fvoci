<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { problemOf } from './problems.ts';

  let {
    available,
    request,
    email: given = '',
    homeHref,
  }: {
    available: Promise<boolean>;
    request: (email: string) => Promise<void>;
    /** The address the form starts with: the one a link gave back, say. */
    email?: string;
    homeHref: string;
  } = $props();
  // Filled in once; what the reader types from then on is theirs.
  // svelte-ignore state_referenced_locally
  let email = $state(given);
  let working = $state(false);
  let done = $state(false);
  let problem = $state<string | undefined>(undefined);

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working || done) return;
    working = true;
    problem = undefined;
    try {
      await request(email.trim());
      done = true;
      email = '';
    } catch (error) {
      problem = problemOf(error);
    } finally {
      working = false;
    }
  }
</script>

<main class="grid min-h-dvh place-items-center p-4">
  <div class="grid w-full max-w-sm gap-4">
    <div class="text-center">
      <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      <h1 class="mt-2 text-lg font-medium">{m.reset_request_title()}</h1>
    </div>
    {#await available}
      <p role="status">{m.account_working()}</p>
    {:then enabled}
      {#if !enabled}
        <p role="status">{m.reset_unavailable()}</p>
      {:else if done}
        <p role="status">{m.reset_request_done()}</p>
      {:else}
        <form
          class="grid gap-4"
          onsubmit={(event) => void submit(event)}
          data-testid="request-password-form"
        >
          <div class="grid gap-1.5">
            <Label for="recovery-email">{m.account_email()}</Label>
            <Input
              id="recovery-email"
              type="email"
              autocomplete="email"
              bind:value={email}
              required
              disabled={working}
            />
          </div>
          {#if problem}<p class="text-sm text-destructive" role="alert">{problem}</p>{/if}
          <Button type="submit" disabled={working} class="roomy:min-h-touch"
            >{m.reset_request_submit()}</Button
          >
        </form>
      {/if}
    {:catch error}
      <p role="alert">{problemOf(error)}</p>
    {/await}
    <a href={homeHref} class="text-center text-sm underline underline-offset-4"
      >{m.reset_sign_in()}</a
    >
  </div>
</main>
