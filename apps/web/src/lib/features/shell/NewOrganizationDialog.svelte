<!--
  Starts an organization, from the workspace switcher: asks for its name, and stays
  open while the server makes it. A refusal is said in the dialog: what is
  wrong with the name beside the field, or that the reader may start no more
  organizations, or none on this server (`newOrganizationProblemOf`). Each opening starts empty.
-->
<script lang="ts">
  import { tick } from 'svelte';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';
  import { Input } from '$lib/ui/input/index.js';
  import { Label } from '$lib/ui/label/index.js';

  import { newOrganizationProblemOf, type NewOrganizationProblem } from './new-organization.ts';

  interface Props {
    open: boolean;
    /** Starts the organization and goes to it; a refusal rejects. The dialog closes once it resolves. */
    onCreate: (name: string) => Promise<void>;
  }

  let { open = $bindable(), onCreate }: Props = $props();

  const uid = $props.id();
  let name = $state('');
  let working = $state(false);
  let problem = $state.raw<NewOrganizationProblem | undefined>(undefined);
  let field = $state<HTMLInputElement | null>(null);

  function forget(): void {
    name = '';
    problem = undefined;
  }

  async function submit(event: SubmitEvent): Promise<void> {
    event.preventDefault();
    if (working) return;
    const trimmed = name.trim();
    if (trimmed === '') {
      problem = { text: m.account_field_required(), ofName: true };
      field?.focus();
      return;
    }
    working = true;
    problem = undefined;
    try {
      await onCreate(trimmed);
      open = false;
      forget();
    } catch (error) {
      problem = newOrganizationProblemOf(error);
      // The field was out of use while it worked: back in use first, then the reader goes on from it.
      working = false;
      await tick();
      field?.focus();
    } finally {
      working = false;
    }
  }
</script>

<Dialog.Root
  bind:open
  onOpenChange={(next: boolean) => {
    if (!next) forget();
  }}
>
  <Dialog.Content data-testid="new-organization-dialog">
    <form class="grid gap-4" onsubmit={(event: SubmitEvent) => void submit(event)}>
      <Dialog.Header>
        <Dialog.Title>{m.org_new()}</Dialog.Title>
        <Dialog.Description>{m.org_new_description()}</Dialog.Description>
      </Dialog.Header>
      <div class="grid gap-1.5">
        <Label for="{uid}-name">{m.account_organization_name()}</Label>
        <Input
          bind:ref={field}
          bind:value={name}
          id="{uid}-name"
          data-testid="new-organization-name"
          autocomplete="organization"
          enterkeyhint="done"
          disabled={working}
          aria-invalid={problem?.ofName ? 'true' : undefined}
          aria-describedby={problem ? `${uid}-problem ${uid}-hint` : `${uid}-hint`}
          class="roomy:h-touch roomy:text-base"
        />
        <p id="{uid}-hint" class="text-xs text-muted-foreground">
          {m.account_organization_name_hint()}
        </p>
      </div>
      {#if problem}
        <p
          id="{uid}-problem"
          class="text-sm text-destructive"
          role="alert"
          data-testid="new-organization-problem"
        >
          {problem.text}
        </p>
      {/if}
      <Dialog.Footer>
        <Button
          type="button"
          variant="outline"
          class="roomy:min-h-touch"
          onclick={() => {
            open = false;
            forget();
          }}
        >
          {m.shared_cancel()}
        </Button>
        <Button
          type="submit"
          class="roomy:min-h-touch"
          disabled={working}
          data-testid="new-organization-submit"
        >
          {working ? m.account_working() : m.org_new_submit()}
        </Button>
      </Dialog.Footer>
    </form>
  </Dialog.Content>
</Dialog.Root>
