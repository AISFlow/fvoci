<script lang="ts">
  import { drawEmbeds } from '$lib/editor/embeds.ts';
  import { renderBody } from '$lib/editor/render.ts';
  import '$lib/editor/editor.css';

  let {
    kind = 'diagram',
    source = 'flowchart LR\n A[Example] --> B[Done]',
    count = 1,
    draw = true,
  }: { kind?: 'diagram' | 'math'; source?: string; count?: number; draw?: boolean } = $props();
  const body = $derived.by(() => {
    const node =
      kind === 'diagram'
        ? {
            type: 'codeBlock',
            attrs: { language: 'mermaid' },
            content: [{ type: 'text', text: source }],
          }
        : { type: 'blockMath', attrs: { latex: source } };
    const result = renderBody({ type: 'doc', content: Array.from({ length: count }, () => node) });
    return result.ok ? [...result.fragment.childNodes] : [];
  });
</script>

<div
  class="doc-body max-w-3xl"
  data-testid="embed-example"
  {@attach (element: HTMLElement) => {
    element.replaceChildren(...body.map((node) => node.cloneNode(true)));
    return draw ? drawEmbeds(element) : undefined;
  }}
></div>
