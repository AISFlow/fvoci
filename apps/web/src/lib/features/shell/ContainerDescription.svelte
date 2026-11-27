<!--
  The description of a board or a wiki, behind "ⓘ" next to its title: the
  title row keeps its room for the title, and the description is read where
  it opens (a popover; a sheet on a compact page). Where there is no
  description there is a button only for whoever may write one, named to
  add it. A narrow pane (a phone) has no room for the button
  beside the title: the top bar opens it from its menu (`open`).

  A reader who may change it (`onChange`) changes it there, in a field: what
  is typed is handed on, once, when the surface closes, whether by a click
  elsewhere, by "Done" (the sheet's own, or the popover's, after the field
  for Tab) or by Ctrl+Enter (⌘Enter on Apple devices); Escape in the panel
  (the field, "Done") puts the description back first. Leaving the board while it is open hands
  it on too, to the board it was open on, and it does not open again when
  the board comes back. While an input method is composing (Hangul, kana)
  Escape and Enter are its own: they neither close the surface nor change
  what is kept.
-->
<script lang="ts">
  import InfoIcon from '@lucide/svelte/icons/info';
  import { onDestroy } from 'svelte';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { PickerPresentation } from '$lib/components/picker-surface/default-presentation.svelte.ts';
  import PickerSurface from '$lib/components/picker-surface/PickerSurface.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isComposing, primaryModifier } from '$lib/keys.ts';
  import { Button } from '$lib/ui/button/index.js';
  import { Textarea } from '$lib/ui/textarea/index.js';

  interface Props {
    /** Bindable: the top bar's menu opens it where the button is away. */
    open?: boolean;
    description: string;
    /** Without it the description is only read. */
    onChange?: (description: string) => void;
  }

  let { open = $bindable(false), description, onChange }: Props = $props();

  const presentation = new PickerPresentation();
  /**
   * What the reader has typed and not yet handed on, with the description it
   * began from and the change it goes to, both taken as the typing began: the
   * board or wiki it was typed on, and handing it on reads nothing of the
   * page (it may be going, and the top bar with what it shows).
   */
  let draft = $state.raw<{ text: string; from: string; save?: Props['onChange'] }>();

  function commit(): void {
    if (!draft) return;
    const { text, from, save } = draft;
    draft = undefined;
    const next = text.trim();
    if (next !== from) save?.(next);
  }

  /** Hands on what was typed and closes: "Done", and Ctrl+Enter (⌘Enter) in the field. */
  function done(): void {
    commit();
    open = false;
  }

  // Gone while open (the reader went to another board, or the description was emptied
  // elsewhere): what was typed is handed on all the same, and the top bar no longer
  // counts it open. Nothing of the top bar is read here: its state read from here was
  // left stale for good (Svelte 5.57.3), so `open` is only written.
  onDestroy(() => {
    commit();
    open = false;
  });

  /**
   * The keys of the panel, wherever its focus is (the field, "Done"). For
   * `{@attach}`: a listener of its own, before the surface's on the document.
   */
  function keys(panel: HTMLElement): () => void {
    const onkeydown = (event: KeyboardEvent): void => {
      if (event.key === 'Enter' && primaryModifier(event) && !isComposing(event)) {
        event.preventDefault();
        done();
        return;
      }
      if (event.key !== 'Escape') return;
      // An input method's Escape stops here, short of the surface.
      if (isComposing(event)) event.stopPropagation();
      // Otherwise it goes on to the surface, which closes; nothing typed is kept.
      else draft = undefined;
    };
    panel.addEventListener('keydown', onkeydown);
    return () => {
      panel.removeEventListener('keydown', onkeydown);
    };
  }

  const name = $derived(description ? m.shell_description() : m.shell_description_add());
</script>

<PickerSurface
  bind:open={
    () => open,
    (next: boolean) => {
      open = next;
      if (!next) commit();
    }
  }
  presentation={presentation.current}
  title={m.shell_description()}
>
  {#snippet trigger({ props }: { props: Record<string, unknown> })}
    <IconButton
      {...props}
      label={name}
      data-testid="container-description"
      class="shrink-0 text-muted-foreground @max-columns/pane:hidden"
    >
      <InfoIcon />
    </IconButton>
  {/snippet}
  <div
    class="w-80 max-w-full p-1 group-data-[presentation=sheet]/surface:w-full"
    data-testid="container-description-panel"
    {@attach keys}
  >
    {#if onChange}
      <Textarea
        value={draft?.text ?? description}
        aria-label={m.shell_description()}
        maxlength={2000}
        rows={3}
        data-picker-focus
        data-testid="container-description-field"
        class="max-h-80 roomy:text-base"
        oninput={(event: Event & { currentTarget: HTMLTextAreaElement }) => {
          const text = event.currentTarget.value;
          draft = { text, from: draft?.from ?? description, save: draft?.save ?? onChange };
        }}
      />
      <!-- The sheet has its own "Done" in its head. -->
      <div class="flex justify-end pt-1 group-data-[presentation=sheet]/surface:hidden">
        <Button
          size="sm"
          variant="ghost"
          class="roomy:min-h-touch"
          data-testid="container-description-done"
          onclick={done}
        >
          {m.ui_done()}
        </Button>
      </div>
    {:else}
      <p
        class="max-h-80 overflow-y-auto text-sm break-words whitespace-pre-line"
        data-testid="container-description-text"
      >
        {description}
      </p>
    {/if}
  </div>
</PickerSurface>
