<!--
  A folded list at the foot of the sidebar, named with how many it holds: the
  boards hidden from it, or the open ones the reader has neither made nor
  joined. Unfolded, a link to each, with a command at its end where there is
  one (`tool`). It is folded until it is unfolded by hand; one that unfolds by
  itself (`unfoldWhenActive`) is unfolded while it holds the container shown,
  until it is folded by hand. The hidden boards are not: a board hidden as it
  is shown leaves the sidebar, as any other does.
-->
<script lang="ts">
  import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
  import type { Snippet } from 'svelte';

  import type { Container } from '$lib/logic/wire.ts';

  import ContainerIcon from './ContainerIcon.svelte';
  import { namedAs, shownTitle } from './shown-title.ts';

  interface Props {
    testId: string;
    /** Its name, with how many it holds. */
    label: string;
    containers: readonly Container[];
    /** The container shown. */
    containerId: string | undefined;
    locale: string;
    containerHref: (container: Container) => string;
    onOpenContainer: (container: Container) => void;
    tool?: Snippet<[Container]>;
    /** Unfolded by itself while it holds the container shown, until it is folded by hand. */
    unfoldWhenActive?: boolean;
  }

  let {
    testId,
    label,
    containers,
    containerId,
    locale,
    containerHref,
    onOpenContainer,
    tool,
    unfoldWhenActive = false,
  }: Props = $props();

  let folded = $state<boolean | undefined>(undefined);
  const open = $derived(
    folded === undefined
      ? unfoldWhenActive && containers.some((one) => one.id === containerId)
      : !folded,
  );

  function follow(event: MouseEvent, container: Container): void {
    if (event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
      return;
    }
    event.preventDefault();
    onOpenContainer(container);
  }
</script>

<div class="pt-2" data-testid={testId}>
  <button
    type="button"
    aria-expanded={open}
    class="flex min-h-8 w-full items-center gap-1 rounded-md py-1 pl-1 text-left text-xs font-medium text-muted-foreground hover:bg-sidebar-accent roomy:min-h-touch"
    onclick={() => {
      folded = open;
    }}
  >
    <ChevronRightIcon
      class={['size-4 shrink-0 transition-transform', open && 'rotate-90']}
      aria-hidden="true"
    />
    <span class="min-w-0 flex-1 truncate">{label}</span>
  </button>
  {#if open}
    <ul>
      {#each containers as container (container.id)}
        {@const title = shownTitle(container)}
        <li class="flex items-center gap-1">
          <a
            href={containerHref(container)}
            aria-current={container.id === containerId ? 'page' : undefined}
            aria-label={namedAs(container.kind, title)}
            class="flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-md py-1 pl-2 text-muted-foreground hover:bg-sidebar-accent roomy:min-h-touch"
            onclick={(event: MouseEvent) => {
              follow(event, container);
            }}
          >
            <ContainerIcon icon={container.icon} kind={container.kind} {title} {locale} />
            <span class="min-w-0 flex-1 truncate">{title}</span>
          </a>
          {@render tool?.(container)}
        </li>
      {/each}
    </ul>
  {/if}
</div>
