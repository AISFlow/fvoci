<!--
  The bubble over what is chosen in a body (bubble-place.ts places it).

  Over chosen text: bold, italic, strikethrough, code, a link and a formula
  (format-actions.ts), each pressed again to take it off. "Link" asks for
  the address in the bubble itself (LinkField.svelte). Over a chosen image:
  "View larger" (ImageLightbox.svelte, which the caller opens).

  A press anywhere on it but the address keeps the focus and the selection in
  the editor, a press of a button that cannot be pressed now too. Tab from
  the editor stops once in the bubble, whose arrows go along it. It stays
  while the focus is in it (`within`), and goes with the selection.
-->
<script lang="ts">
  import LinkIcon from '@lucide/svelte/icons/link';
  import Maximize2Icon from '@lucide/svelte/icons/maximize-2';
  import type { Editor } from '@tiptap/core';
  import { NodeSelection, TextSelection } from '@tiptap/pm/state';
  import { on } from 'svelte/events';

  import { m } from '$lib/i18n/paraglide/messages.js';

  import { focusedFormat, keepEditorFocus, moveAlongBar, tabStop } from './bar-keys.ts';
  import { placeBubble, selectionRect } from './bubble-place.ts';
  import { FORMULA_ACTION, MARK_ACTIONS, type FormatAction } from './format-actions.ts';
  import { lightboxOf, type LightboxImage } from './image-zoom.ts';
  import LinkField from './LinkField.svelte';

  interface Props {
    editor: Editor;
    /** Goes up with every change of the document or the selection. */
    changes: number;
    /** It may show: the editor or the bubble has the focus, and no press is choosing text. */
    shown: boolean;
    /** The editor can be written in now. */
    editable: boolean;
    /** The marks are offered over chosen text (not where the keyboard's bar is). */
    marks: boolean;
    /** Under what is chosen: a touch screen puts its own menu over it. */
    below: boolean;
    /** The focus is in the bubble. Bindable. */
    within?: boolean;
    onZoom: (image: LightboxImage) => void;
  }

  let {
    editor,
    changes,
    shown,
    editable,
    marks,
    below,
    within = $bindable(false),
    onZoom,
  }: Props = $props();

  /** The address of a link is being written. */
  let linking = $state(false);

  /** The image chosen, as it is drawn. */
  function chosenImage(): HTMLImageElement | undefined {
    const { selection } = editor.state;
    if (!(selection instanceof NodeSelection) || selection.node.type.name !== 'image') return;
    const dom = editor.view.nodeDOM(selection.from);
    return dom instanceof HTMLImageElement && dom.hasAttribute('src') ? dom : undefined;
  }

  /** What the bubble is over: chosen text, an image, or nothing; asked again on each change. */
  function kindNow(showing: boolean, ..._changed: unknown[]): 'text' | 'image' | undefined {
    if (!showing) return undefined;
    if (chosenImage()) return 'image';
    const { selection } = editor.state;
    if (!marks || !editable || !(selection instanceof TextSelection) || selection.empty) return;
    const code = selection.$from.parent.type.spec.code || selection.$to.parent.type.spec.code;
    return code ? undefined : 'text';
  }
  const kind = $derived(kindNow(shown, changes));

  /** Each button's state where the selection is now; asked again on each change. */
  function statesNow(over: typeof kind, ..._changed: unknown[]) {
    const of = (action: FormatAction) => ({
      action,
      can: over === 'text' && action.can(editor),
      on: action.active?.(editor) ?? false,
    });
    return {
      marks: MARK_ACTIONS.map(of),
      formula: of(FORMULA_ACTION),
      link: editor.isActive('link'),
    };
  }
  const states = $derived(statesNow(kind, changes));

  /** The mark's button focused last; Tab comes back to it, the arrows go to the rest. */
  let last = $state<string>();
  const stop = $derived(
    tabStop(
      [...states.marks, { action: { id: 'link' }, can: true }, states.formula].map((state) => ({
        id: state.action.id,
        can: state.can,
      })),
      last,
    ),
  );

  /**
   * Placed again as what is chosen changes (`moved`), as the page scrolls or changes its size, and
   * as the bubble does (a refused address is said in it).
   */
  function placed(bubble: HTMLElement, ..._moved: unknown[]): () => void {
    const place = (): void => {
      const target = selectionRect(editor);
      if (target) placeBubble(bubble, target, below);
    };
    place();
    const page = bubble.ownerDocument.defaultView ?? window;
    const scrolled = on(page, 'scroll', place, { capture: true, passive: true });
    const resized = on(page, 'resize', place);
    const sized = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(place);
    sized?.observe(bubble);
    return () => {
      scrolled();
      resized();
      sized?.disconnect();
    };
  }
</script>

{#snippet button(state: { action: FormatAction; can: boolean; on: boolean })}
  <button
    type="button"
    data-format={state.action.id}
    aria-label={state.action.label()}
    title={state.action.label()}
    aria-pressed={state.action.active ? state.on : undefined}
    disabled={!state.can}
    tabindex={state.action.id === stop ? 0 : -1}
    onclick={() => {
      state.action.run(editor);
    }}
  >
    <state.action.icon aria-hidden={true} />
  </button>
{/snippet}

{#if kind}
  <div
    class="doc-bubble"
    role="toolbar"
    tabindex="-1"
    aria-label={m.editor_format_bar()}
    data-bubble={kind}
    data-editor-tool="bubble"
    {@attach (bubble: HTMLElement) => placed(bubble, changes, kind, linking)}
    onmousedown={keepEditorFocus}
    onfocusin={(event) => {
      within = true;
      last = focusedFormat(event) ?? last;
    }}
    onfocusout={(event) => {
      const next = event.relatedTarget;
      if (next instanceof Node && event.currentTarget.contains(next)) return;
      within = false;
      linking = false;
    }}
    onkeydown={(event) => {
      if (!linking) moveAlongBar(event, event.currentTarget);
    }}
  >
    {#if kind === 'image'}
      <button
        type="button"
        class="doc-bubble-word"
        data-format="zoom"
        onclick={() => {
          const image = chosenImage();
          if (image) onZoom(lightboxOf(image));
        }}
      >
        <Maximize2Icon aria-hidden="true" />
        {m.editor_image_zoom()}
      </button>
    {:else if linking}
      <LinkField {editor} ondone={() => (linking = false)} />
    {:else}
      {#each states.marks as state (state.action.id)}
        {@render button(state)}
      {/each}
      <button
        type="button"
        data-format="link"
        aria-label={m.editor_format_link()}
        title={m.editor_format_link()}
        aria-pressed={states.link}
        tabindex={stop === 'link' ? 0 : -1}
        onclick={() => (linking = true)}
      >
        <LinkIcon aria-hidden="true" />
      </button>
      {@render button(states.formula)}
    {/if}
  </div>
{/if}

<style>
  .doc-bubble {
    position: absolute;
    z-index: 40;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.125rem;
    max-width: calc(100vw - 1rem);
    padding: 0.25rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--popover);
    color: var(--popover-foreground);
    box-shadow: 0 0.25rem 1rem color-mix(in oklab, var(--foreground) 14%, transparent);
    font-size: var(--text-sm);
  }
  .doc-bubble:focus {
    outline: none;
  }
  .doc-bubble button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.375rem;
    min-width: 2rem;
    height: 2rem;
    padding: 0 0.375rem;
    border: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  .doc-bubble button:hover:not(:disabled) {
    background: var(--accent);
    color: var(--accent-foreground);
  }
  .doc-bubble button[aria-pressed='true'] {
    background: color-mix(in oklab, var(--ring) 18%, transparent);
    color: var(--foreground);
  }
  .doc-bubble button:disabled {
    opacity: 0.4;
    /* A press goes through to the bubble, which keeps the focus in the editor. */
    pointer-events: none;
  }
  .doc-bubble button:focus-visible {
    outline: 2px solid var(--ring);
    outline-offset: 1px;
  }
  .doc-bubble button :global(svg) {
    width: 1rem;
    height: 1rem;
  }
  .doc-bubble-word {
    padding: 0 0.625rem;
  }
  @media (pointer: coarse) {
    .doc-bubble button {
      min-width: 2.75rem;
      height: 2.75rem;
    }
  }
</style>
