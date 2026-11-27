<!--
  Blocks of two bodies merged by a comparison (logic/body-diff.ts), for
  reading: drawn as `BodyView` draws a body (`drawDiffBlocks`, render-diff.ts,
  over render.ts), with formulas, diagrams, files, references and the colours
  of code drawn into them after, and what changed marked (diff.css).

  Both bodies were accepted by the validator before they were compared
  (compare-bodies.ts); nothing else may be given.
-->
<script lang="ts">
  import type { ClassValue } from 'svelte/elements';

  import type { DiffNode } from '$lib/logic/body-nodes.ts';

  import './editor.css';
  import './diff.css';
  import { CODE_COPY_TEXTS } from './code-copy-texts.ts';
  import { drawCodeBlocksLater } from './code-later.ts';
  import { drawEmbeds } from './embeds.ts';
  import { drawFileBlocks } from './file-view.ts';
  import { attachReferences, type ReferenceDrawing } from './reference-chip.ts';
  import { drawDiffBlocks } from './render-diff.ts';

  interface Props {
    blocks: DiffNode[];
    /** The name of a mentioned user; the identifier is shown when there is none. */
    resolveUser?: (userId: string) => string | undefined;
    /** The accessible name of the checkbox of a task, from the text of the task. */
    taskLabel?: (text: string, checked: boolean) => string;
    /** What the references are drawn from, and how one is opened. */
    references?: ReferenceDrawing;
    class?: ClassValue;
  }

  let { blocks, resolveUser, taskLabel, references, class: className }: Props = $props();

  // A fragment gives its nodes away once; the list can be placed again.
  const nodes = $derived([...drawDiffBlocks(blocks, { resolveUser, taskLabel }).childNodes]);
</script>

<div
  class={['doc-body', className]}
  {@attach (element: HTMLElement) => {
    element.replaceChildren(...nodes);
    drawFileBlocks(element);
    return drawEmbeds(element);
  }}
  {@attach (element: HTMLElement) => attachReferences(element, references, nodes)}
  {@attach (element: HTMLElement) => drawCodeBlocksLater(element, CODE_COPY_TEXTS, nodes)}
></div>
