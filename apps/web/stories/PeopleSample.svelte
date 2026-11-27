<script lang="ts">
  import { onMount } from 'svelte';
  import PeoplePanel from '$lib/features/shell/PeoplePanel.svelte';
  import GuestInvite from '$lib/features/shell/GuestInvite.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';

  import { exampleWorkspace, loadWorkspace } from './workspace.ts';
  import { ExamplePeople, exampleInviteLink, type PeopleMode } from './people.ts';

  let {
    count = 6,
    mode = 'ready',
    guest = false,
  }: { count?: number; mode?: PeopleMode; guest?: boolean } = $props();
  const example = exampleWorkspace({ count: 0 });
  const people = $derived(new ExamplePeople(count, mode));
  const access = $derived({ people, inviteLink: exampleInviteLink });
  onMount(() => {
    void loadWorkspace(example);
    return () => {
      example.workspace.close();
    };
  });
</script>

<div
  class="max-w-xl"
  data-testid="people-example"
  data-fixture-ready={example.workspace.status === 'ready' ? '' : undefined}
>
  {#if example.workspace.status === 'ready'}
    {#key people}
      {#if guest}
        <GuestInvite workspace={example.workspace} {access} containerId="example-board" />
      {:else}
        <PeoplePanel workspace={example.workspace} {people} inviteLink={exampleInviteLink} />
      {/if}
    {/key}
  {:else}
    <p role="status">{m.app_loading()}</p>
  {/if}
</div>
