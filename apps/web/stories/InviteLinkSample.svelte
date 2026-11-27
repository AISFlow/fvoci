<script lang="ts">
  import InviteLink from '$lib/features/shell/InviteLink.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';

  let { long = false }: { long?: boolean } = $props();
</script>

<div class="max-w-md">
  <InviteLink
    link={`https://example.test/invite/${long ? 'example-long-link-'.repeat(20) : 'example-invitation'}`}
    label={m.people_invite_link()}
    note={m.people_invite_note()}
    testId="example-invite-link"
  />
</div>
