<script lang="ts">
  import AccountFields from '$lib/features/account/AccountFields.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';

  let {
    populated = false,
    invalid = false,
    busy = false,
    long = false,
  }: { populated?: boolean; invalid?: boolean; busy?: boolean; long?: boolean } = $props();
  let username = $derived(populated ? 'example-reader' : '');
  let displayName = $derived(
    populated
      ? long
        ? 'Example member · 예시 구성원 '.repeat(10)
        : 'Example member · 예시 구성원'
      : '',
  );
  let email = $derived(populated ? 'example-reader@example.test' : '');
  let password = $state('');
  const problems = $derived(
    invalid
      ? {
          username: m.account_username_taken(),
          displayName: m.account_display_name_too_long(),
          email: m.account_email_invalid(),
          password: m.account_password_too_short(),
        }
      : {},
  );
</script>

<form
  class="max-w-md"
  onsubmit={(event) => {
    event.preventDefault();
  }}
>
  <fieldset disabled={busy} class="grid gap-4" aria-busy={busy}>
    <AccountFields bind:username bind:displayName bind:email bind:password {problems} />
  </fieldset>
</form>
