<!--
  The title of the board, as a field that looks like the heading it is in.

  What is typed is kept here until the field is left or Enter is pressed;
  then it is handed on, once, without the spaces around it. Escape puts the
  title back. While an input method is composing (Hangul, kana) Enter and
  Escape are its own.

  The field is as wide as its text where the browser can do that
  (`field-sizing`), and about as wide where it cannot (`size`).
-->
<script lang="ts">
  import { isComposing } from '$lib/keys.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';

  interface Props {
    title: string;
    onChange: (title: string) => void;
  }

  let { title, onChange }: Props = $props();

  /** What the reader has typed and not yet handed on. */
  let draft = $state<string | undefined>(undefined);
  const value = $derived(draft ?? title);

  function commit(): void {
    if (draft === undefined) return;
    const next = draft.trim();
    draft = undefined;
    if (next !== title) onChange(next);
  }

  function onkeydown(event: KeyboardEvent & { currentTarget: HTMLInputElement }): void {
    if (isComposing(event)) return;
    if (event.key === 'Enter') {
      event.currentTarget.blur();
    } else if (event.key === 'Escape') {
      draft = undefined;
      event.currentTarget.blur();
    }
  }
</script>

<input
  {value}
  type="text"
  data-testid="board-title"
  aria-label={m.shell_board_title()}
  placeholder={m['ViewTitle.untitled-board']()}
  size={Math.max(8, value.length)}
  maxlength={200}
  autocomplete="off"
  enterkeyhint="done"
  class="field-sizing-content max-w-full min-w-0 truncate rounded-md border border-transparent bg-transparent px-1.5 py-0.5 font-semibold outline-none placeholder:font-normal placeholder:text-muted-foreground hover:border-input-border focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 @columns/pane:min-w-24 roomy:min-h-touch"
  oninput={(event) => {
    draft = event.currentTarget.value;
  }}
  onblur={commit}
  {onkeydown}
/>
