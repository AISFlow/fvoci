<script lang="ts">
  import NameDialog from '$lib/features/organization/NameDialog.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';

  let { initial = '', startOpen = true }: { initial?: string; startOpen?: boolean } = $props();
  let open = $derived(startOpen);
  let name = $derived(initial);
</script>

<Button
  onclick={() => {
    open = true;
  }}>{m.org_rename()}</Button
>
<output data-testid="submitted-name" class="m-4 break-all">{name}</output>
<NameDialog
  bind:open
  title={m.org_rename()}
  initial={name}
  onSubmit={(value: string) => {
    name = value;
  }}
/>
