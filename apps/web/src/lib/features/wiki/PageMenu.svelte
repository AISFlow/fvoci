<!--
  The menu at the head of a page: what can be done with the page shown. Its
  link to copy (`href`, which a body pastes as a reference to it), and its
  versions, which whoever reads the wiki reads (`onVersions`).
-->
<script lang="ts">
  import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
  import HistoryIcon from '@lucide/svelte/icons/history';
  import LinkIcon from '@lucide/svelte/icons/link';

  import { copyCardLink } from '$lib/components/card-number/card-link.ts';

  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import ActionMenu from '$lib/components/menu/ActionMenu.svelte';
  import type { MenuAction } from '$lib/components/menu/actions.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { WikiPage } from '$lib/state/index.ts';

  interface Props {
    page: WikiPage;
    /** The address of the page, from the root of the site. */
    href: string;
    /** Opens the versions kept of the page's body. */
    onVersions: () => void;
  }

  let { page, href, onVersions }: Props = $props();

  /** Said for a while once the link is copied. */
  let copied = $state(false);

  const title = $derived(page.title || m.wiki_page_untitled());

  async function copyLink(): Promise<void> {
    try {
      await copyCardLink(new URL(href, location.href).href, { title });
    } catch {
      // No clipboard here (a page that is not served securely): nothing was copied, so nothing is said.
      return;
    }
    copied = true;
    setTimeout(() => {
      copied = false;
    }, 2000);
  }

  const actions = $derived<MenuAction[]>([
    {
      id: 'copy-link',
      label: m['CardActionsMenu.copyLink'](),
      icon: LinkIcon,
      run: () => void copyLink(),
    },
    { id: 'versions', label: m.card_versions(), icon: HistoryIcon, run: onVersions },
  ]);
</script>

<span class="text-xs text-muted-foreground" role="status" data-testid="page-link-copied">
  {copied ? m['CardActionsMenu.copiedLink']() : ''}
</span>

<ActionMenu title={m.wiki_page_more({ title })} subtitle={title} align="end" {actions}>
  {#snippet trigger(props: Record<string, unknown>)}
    <IconButton {...props} label={m.wiki_page_more({ title })} data-testid="page-menu">
      <EllipsisIcon aria-hidden="true" />
    </IconButton>
  {/snippet}
</ActionMenu>
