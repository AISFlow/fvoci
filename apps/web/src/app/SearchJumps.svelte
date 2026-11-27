<!--
  "바로 이동" in the search: the boards, wikis, diaries and courses to go to at
  once (`searchJumps`), each a link (a middle click or ⌘/Ctrl and a click
  open it in a new tab, as any link does; a plain click goes there in this
  one). They are not options of the list of what was found, whose keys stay
  its own: Tab reaches them from the field.
-->
<script lang="ts">
  import ContainerIcon from '$lib/features/shell/ContainerIcon.svelte';
  import { shownTitle } from '$lib/features/shell/shown-title.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { Container } from '$lib/logic/wire.ts';

  interface Props {
    containers: readonly Container[];
    locale: string;
    hrefOf: (container: Container) => string;
    onJump: (container: Container) => void;
  }

  let { containers, locale, hrefOf, onJump }: Props = $props();

  const uid = $props.id();

  function follow(event: MouseEvent, container: Container): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onJump(container);
  }
</script>

<section class="grid gap-0.5 pb-2" aria-labelledby="{uid}-title" data-testid="search-jumps">
  <h3 id="{uid}-title" class="px-2 pb-0.5 text-xs font-medium text-muted-foreground">
    {m.search_jumps()}
  </h3>
  <ul class="grid gap-0.5">
    {#each containers as container (container.id)}
      {@const title = shownTitle(container)}
      <li>
        <a
          href={hrefOf(container)}
          class="flex min-h-9 items-center gap-2.5 rounded-md px-2 text-sm outline-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground focus-visible:ring-3 focus-visible:ring-ring/50 roomy:min-h-touch"
          data-search-jump={container.id}
          onclick={(event) => {
            follow(event, container);
          }}
        >
          <ContainerIcon icon={container.icon} {title} {locale} kind={container.kind} />
          <span class="min-w-0 truncate">{title}</span>
        </a>
      </li>
    {/each}
  </ul>
</section>
