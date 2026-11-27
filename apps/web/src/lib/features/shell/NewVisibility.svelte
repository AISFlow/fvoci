<!--
  Who finds a new thing in a shared space: everyone who works there (open, the
  default) or only those it is shared with.
-->
<script lang="ts">
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ContainerVisibility } from '$lib/logic/wire.ts';

  interface Props {
    visibility: ContainerVisibility;
    workspaceName: string;
  }

  let { visibility = $bindable(), workspaceName }: Props = $props();

  const uid = $props.id();
  const CHOICES: readonly { value: ContainerVisibility; label: () => string }[] = $derived([
    { value: 'open', label: () => m.new_visibility_open({ workspace: workspaceName }) },
    { value: 'private', label: m.new_visibility_private },
  ]);
</script>

<fieldset class="grid gap-1.5">
  <legend class="mb-1 text-sm font-medium">{m.new_visibility_label()}</legend>
  {#each CHOICES as choice (choice.value)}
    <label
      class="flex min-h-8 items-start gap-3 text-sm roomy:min-h-touch"
      for="{uid}-{choice.value}"
    >
      <input
        id="{uid}-{choice.value}"
        type="radio"
        name="{uid}-visibility"
        value={choice.value}
        bind:group={visibility}
        data-testid="new-visibility-{choice.value}"
        class="mt-0.5 size-4 accent-primary"
      />
      <span>{choice.label()}</span>
    </label>
  {/each}
</fieldset>
