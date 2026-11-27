<!--
  The search of cards and pages in one of its states, over nothing: what it
  finds is made up here (`answer`), and going to a hit goes nowhere.
-->
<script lang="ts">
  import { SearchSession } from '$lib/features/search/search-session.svelte.ts';
  import SearchDialog from '$lib/features/search/SearchDialog.svelte';
  import { BackendError, type SearchHit } from '$lib/state/backend.ts';

  type Answer = 'found' | 'none' | 'unavailable' | 'empty';

  let { answer = 'found' }: { answer?: Answer } = $props();

  const HITS: SearchHit[] = [
    {
      id: 'example-card',
      kind: 'card',
      containerId: 'example-board',
      containerKind: 'board',
      title: 'Refund policy review · 환불 규정 검토',
      snippet: [
        { text: 'Customers asked about the ', matched: false },
        { text: 'refund', matched: true },
        { text: ' window; 고객이 환불 기간을 물었다. <script> stays text.', matched: false },
      ],
    },
    {
      id: 'example-page',
      kind: 'page',
      containerId: 'example-wiki',
      containerKind: 'wiki',
      title: '',
      snippet: [],
    },
    {
      id: 'example-unknown',
      kind: 'card',
      containerId: 'example-elsewhere',
      containerKind: 'board',
      title: 'A card of a board the sidebar does not know, with a title long enough to be cut',
      snippet: [
        { text: 'refund', matched: true },
        { text: ` ${'and more words '.repeat(12)}`, matched: false },
      ],
    },
  ];
  const NAMES: Record<string, string> = {
    'example-board': 'Support · 지원',
    'example-wiki': 'Handbook · 안내서',
  };

  const made = (kind: Answer): SearchSession => {
    const session = new SearchSession({
      find: () => {
        if (kind === 'unavailable') {
          return Promise.reject(new BackendError(503, { reason: 'search_unavailable' }));
        }
        return Promise.resolve(kind === 'none' ? [] : HITS);
      },
    });
    if (kind !== 'empty') session.type('refund');
    return session;
  };
  const session = $derived(made(answer));
  let open = $state(true);
</script>

<p class="text-muted-foreground">The search of cards and pages, open.</p>
<SearchDialog
  bind:open
  {session}
  containerOf={(hit: SearchHit) => NAMES[hit.containerId] ?? ''}
  hrefOf={(hit: SearchHit) => `#${hit.kind}-${hit.id}`}
  onGo={() => undefined}
  returnTo={() => undefined}
  locale="en"
/>
