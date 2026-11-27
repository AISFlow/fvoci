<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isBackendError } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';
  import { formProblemsOf } from './problems.ts';

  let {
    token,
    homeHref,
    complete,
  }: {
    token: string;
    homeHref: string;
    complete: (token: string, password: string) => Promise<void>;
  } = $props();

  let password = $state('');
  let working = $state(false);
  let done = $state(false);
  let problem = $state<string | undefined>(undefined);

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working || done) return;
    working = true;
    problem = undefined;
    try {
      await complete(token, password);
      password = '';
      done = true;
      history.replaceState(null, '', location.pathname);
    } catch (error) {
      if (isBackendError(error) && error.errors.some((entry) => entry.pointer === '/token')) {
        problem = m.reset_invalid();
      } else {
        const refused = formProblemsOf(error, { '/password': 'password' });
        problem = refused.fields.password ?? refused.problem;
      }
    } finally {
      working = false;
    }
  }
</script>

<main class="grid min-h-dvh place-items-center p-4">
  <div class="grid w-full max-w-sm gap-4">
    <div class="text-center">
      <p class="text-2xl font-semibold tracking-tight" translate="no">{m.app_name()}</p>
      <h1 class="mt-2 text-lg font-medium">{m.reset_title()}</h1>
    </div>
    {#if done}
      <p role="status">{m.reset_done()}</p>
    {:else if !token}
      <p role="alert">{m.reset_invalid()}</p>
    {:else}
      <form
        class="grid gap-4"
        onsubmit={(event) => void submit(event)}
        data-testid="reset-password-form"
      >
        <div class="grid gap-1.5">
          <Label for="reset-password">{m.reset_password()}</Label>
          <Input
            id="reset-password"
            type="password"
            autocomplete="new-password"
            bind:value={password}
            required
            disabled={working}
            aria-describedby={problem ? 'reset-problem' : undefined}
          />
        </div>
        {#if problem}<p id="reset-problem" class="text-sm text-destructive" role="alert">
            {problem}
          </p>{/if}
        <Button type="submit" disabled={working} class="roomy:min-h-touch"
          >{m.reset_submit()}</Button
        >
      </form>
    {/if}
    <a href={homeHref} class="text-center text-sm underline underline-offset-4"
      >{m.reset_sign_in()}</a
    >
  </div>
</main>
