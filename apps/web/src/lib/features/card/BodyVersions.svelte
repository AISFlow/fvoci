<!--
  The versions kept of a card's body, in a dialog the card's menu opens for
  the board's members: the list (when each was kept, and who wrote the body
  up to it), and the body of the one chosen, only to read. Who edits the
  board can go back to it, after being asked: the card's body becomes that
  version for everyone, and the dialog closes on the card. The version the
  card has now offers nothing to go back to.

  Wide, the list stays beside the body; narrow, the body takes the list's
  place, and a back arrow (and the back button) returns to it. The focus
  follows: to the arrow when the list gives way, and back to the version
  that was read.

  A wiki's page has versions of its body as a card has: they are shown by
  this too (`of="page"`), with the wiki in place of the board, in the page's
  own words.

  The version read shows what it changed from the version before it, or is
  compared with the body now or another version, or shown as it is
  (VersionCompare.svelte).

  Each version says how it was kept, in words: by hand, on its own, or as a
  version gone back to. Who edits can keep one now, from the button over the
  list or with ⌘S or Ctrl+S in the dialog (the browser's dialog to save the
  page never opens there): the list is read again in place, and what came of
  it is said.
-->
<script lang="ts">
  import { tick } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';

  import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
  import SaveIcon from '@lucide/svelte/icons/save';
  import XIcon from '@lucide/svelte/icons/x';

  import Avatar from '$lib/components/avatar/Avatar.svelte';
  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { isSaveKey, shortcutKeys, shortcutLabel } from '$lib/keys.ts';
  import { closeOnBack } from '$lib/layout/back.svelte.ts';
  import { displayDateTime, relativeTime } from '$lib/logic/format.ts';
  import type { BoardStore, BodyVersion, BodyVersionCause } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';
  import * as Dialog from '$lib/ui/dialog/index.js';

  import { KEPT_SAID } from './body-notes.ts';
  import { bodyReferences } from './body-references.ts';
  import type { VersionsReading } from './body-versions.svelte.ts';
  import VersionCompare from './VersionCompare.svelte';

  interface Props {
    open: boolean;
    /** Made when the dialog opens: a new reading each time. */
    reading: VersionsReading | undefined;
    /** The board, or the wiki: who may go back to a version. */
    container: Pick<BoardStore, 'workspace' | 'canEdit'>;
    /** Whose body it is, for the words it says: a card's, a wiki's page's, a course's unit's, or a diary entry's. */
    of?: 'card' | 'page' | 'unit' | 'entry';
  }

  let { open = $bindable(), reading, container, of = 'card' }: Props = $props();

  /** What is said of the card, the page, the unit, or the diary entry. */
  const words = $derived(
    of === 'page'
      ? {
          noBody: m.page_version_no_body,
          invalid: m.wiki_body_invalid,
          isCurrent: m.page_version_is_current,
          restoreDescription: m.page_version_restore_description,
        }
      : of === 'unit'
        ? {
            noBody: m.unit_version_no_body,
            invalid: m.course_body_invalid,
            isCurrent: m.unit_version_is_current,
            restoreDescription: m.unit_version_restore_description,
          }
        : of === 'entry'
          ? {
              noBody: m.entry_version_no_body,
              invalid: m.diary_body_invalid,
              isCurrent: m.entry_version_is_current,
              restoreDescription: m.entry_version_restore_description,
            }
          : {
              noBody: m.card_version_no_body,
              invalid: m.card_body_invalid,
              isCurrent: m.card_version_is_current,
              restoreDescription: m.card_version_restore_description,
            },
  );

  const references = $derived(bodyReferences(container));

  /** How many faces a version shows of those who wrote it. */
  const FACES = 3;

  let asking = $state(false);

  const workspace = $derived(container.workspace);
  const context = $derived({
    locale: workspace.locale,
    timeZone: workspace.regional.timeZone,
    hourCycle: workspace.regional.hourCycle,
  });
  const names = $derived(new Intl.ListFormat(workspace.locale, { type: 'conjunction' }));
  const chosen = $derived(reading?.chosen);
  /** Whether the card has the chosen body now, as its reading says, or the list before it. */
  const current = $derived(reading?.content?.isCurrent ?? chosen?.isCurrent ?? false);

  const resolveUser = (userId: string): string =>
    workspace.userName(userId) ?? m['centerPanel.unknown-user']();
  const whoWrote = (version: BodyVersion): string => names.format(version.editors.map(resolveUser));
  const whenKept = (version: BodyVersion): string => displayDateTime(version.keptAt, context);
  const CAUSES: Record<BodyVersionCause, () => string> = {
    auto: m.card_version_cause_auto,
    manual: m.card_version_cause_manual,
    restored: m.card_version_cause_restored,
    agent: m.card_version_cause_agent,
  };
  /** How it was kept; an agent's version by the name of its token ("Agent · …"). */
  const causeOf = (version: BodyVersion): string =>
    version.cause === 'agent' && version.agentTokenName
      ? m.agent_badge({ name: version.agentTokenName })
      : CAUSES[version.cause]();

  /** The keys that keep a version, in the dialog: as its button, for who edits. */
  function keyInDialog(event: KeyboardEvent): void {
    if (!isSaveKey(event)) return;
    event.preventDefault();
    if (reading && container.canEdit && !event.repeat) void reading.keep();
  }

  /** Narrow: the body of a version takes the place of the list (Tailwind's `sm`). */
  const narrow = new MediaQuery('(max-width: 39.99rem)');

  // The back button closes it, as the other dialogs: the card under it stays.
  closeOnBack(
    () => open,
    () => {
      open = false;
    },
  );
  /** A version is being read: whichever, so that a list read again (a version kept) is no change. */
  const readingOne = $derived(chosen !== undefined);
  // Narrow, with a version open, it goes back to the list first.
  closeOnBack(
    () => open && narrow.current && readingOne,
    () => reading?.leave(),
  );

  /** The dialog a row or the back arrow is in: where the focus is sent on within. */
  const dialogOf = (event: MouseEvent): Element | null =>
    (event.currentTarget as HTMLElement).closest('[data-testid="body-versions"]');

  async function read(event: MouseEvent, version: BodyVersion): Promise<void> {
    const row = event.currentTarget as HTMLElement;
    const dialog = dialogOf(event);
    void reading?.choose(version);
    await tick();
    // The list gave way to the body, and the row that had the focus with it.
    if (row.offsetParent === null) {
      dialog?.querySelector<HTMLElement>('[data-testid="body-versions-back"]')?.focus();
    }
  }

  async function backToList(event: MouseEvent): Promise<void> {
    const dialog = dialogOf(event);
    const left = chosen?.version;
    reading?.leave();
    await tick();
    dialog?.querySelector<HTMLElement>(`[data-version="${String(left)}"]`)?.focus();
  }

  async function restore(): Promise<void> {
    if (await reading?.restore()) open = false;
  }
</script>

<Dialog.Root bind:open>
  <Dialog.Content
    class="flex h-[min(90dvh,44rem)] flex-col gap-0 p-0 [--doc-surface:var(--popover)] sm:max-w-3xl"
    showCloseButton={false}
    data-testid="body-versions"
    onkeydown={keyInDialog}
  >
    <Dialog.Header class="flex-row items-center gap-2 border-b p-4">
      {#if chosen}
        <IconButton
          label={m.card_versions_list()}
          class="sm:hidden"
          data-testid="body-versions-back"
          onclick={backToList}
        >
          <ArrowLeftIcon aria-hidden="true" />
        </IconButton>
      {/if}
      <div class="min-w-0 flex-1">
        <Dialog.Title>{m.card_versions_title()}</Dialog.Title>
        <Dialog.Description class={[chosen && 'max-sm:sr-only']}>
          {m.card_versions_description()}
        </Dialog.Description>
      </div>
      <Dialog.Close>
        {#snippet child({ props }: { props: Record<string, unknown> })}
          <IconButton {...props} label={m.ui_close()}>
            <XIcon aria-hidden="true" />
          </IconButton>
        {/snippet}
      </Dialog.Close>
    </Dialog.Header>

    {#if reading}
      <div class="flex min-h-0 flex-1">
        <div
          class={[
            'min-h-0 w-full overflow-y-auto overscroll-contain sm:w-72 sm:shrink-0 sm:border-e',
            chosen && 'max-sm:hidden',
          ]}
        >
          {#if container.canEdit}
            <div class="grid justify-items-start gap-1.5 border-b p-3">
              <!-- Not disabled while a version is kept: the focus stays on it. -->
              <Button
                variant="outline"
                size="sm"
                class="roomy:min-h-touch"
                aria-keyshortcuts={shortcutKeys('S')}
                data-testid="body-versions-keep"
                onclick={() => void reading.keep()}
              >
                <SaveIcon aria-hidden="true" />
                {m.card_version_keep_now()}
                <kbd class="font-sans text-xs text-muted-foreground">{shortcutLabel('S')}</kbd>
              </Button>
              <p
                class="text-xs text-muted-foreground"
                role="status"
                data-testid="body-versions-kept"
              >
                {reading.kept ? KEPT_SAID[reading.kept]() : ''}
              </p>
            </div>
          {/if}
          {#if reading.status === 'loading'}
            <p class="p-4 text-muted-foreground" role="status">{m.app_loading()}</p>
          {:else if reading.status === 'failed'}
            <div class="grid justify-items-start gap-2 p-4">
              <p role="alert">{m.card_versions_failed()}</p>
              <Button variant="outline" class="roomy:min-h-touch" onclick={() => reading.read()}>
                {m.app_retry()}
              </Button>
            </div>
          {:else if reading.versions.length === 0}
            <p class="p-4 text-muted-foreground" data-testid="body-versions-empty">
              {m.card_versions_empty()}
            </p>
          {:else}
            <ol class="grid p-1.5" data-testid="body-versions-list">
              {#each reading.versions as version (version.version)}
                {@const when = whenKept(version)}
                <li>
                  <button
                    type="button"
                    class="grid w-full gap-1 rounded-md px-2.5 py-2 text-start outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring aria-[current=true]:bg-muted roomy:min-h-touch"
                    aria-current={chosen?.version === version.version}
                    disabled={reading.restoring}
                    data-version={version.version}
                    onclick={(event) => read(event, version)}
                  >
                    <span class="flex items-center gap-2">
                      <time class="font-medium" datetime={new Date(version.keptAt).toISOString()}>
                        {relativeTime(version.keptAt, workspace.locale)}
                      </time>
                      {#if version.isCurrent}
                        <span
                          class="rounded-sm bg-primary/10 px-1.5 py-0.5 text-xs font-medium text-primary"
                          data-testid="body-version-current"
                        >
                          {m.card_version_current()}
                        </span>
                      {/if}
                    </span>
                    <span class="text-xs text-muted-foreground">
                      {when} ·
                      <span
                        class={version.cause === 'agent'
                          ? 'rounded-sm bg-muted px-1 font-medium text-foreground'
                          : undefined}
                        data-testid="body-version-cause"
                      >
                        {causeOf(version)}
                      </span>
                    </span>
                    {#if version.editors.length > 0}
                      <span class="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
                        <span class="flex shrink-0">
                          {#each version.editors.slice(0, FACES) as userId, index (userId)}
                            <Avatar
                              name={resolveUser(userId)}
                              colorKey={userId}
                              locale={workspace.locale}
                              decorative
                              class={['size-5 ring-2 ring-popover', index > 0 && '-ms-1']}
                            />
                          {/each}
                        </span>
                        <span class="truncate" data-testid="body-version-editors">
                          {whoWrote(version)}
                        </span>
                      </span>
                    {/if}
                  </button>
                </li>
              {/each}
            </ol>
          {/if}
        </div>

        <section
          class={['min-h-0 min-w-0 flex-1 flex-col', chosen ? 'flex' : 'hidden sm:flex']}
          aria-label={chosen ? m.card_version_read({ when: whenKept(chosen) }) : undefined}
          data-testid="body-version"
        >
          {#if !chosen}
            <p class="m-auto max-w-xs p-6 text-center text-muted-foreground">
              {reading.versions.length > 0 ? m.card_versions_pick() : ''}
            </p>
          {:else}
            <p class="border-b px-4 py-2.5 text-xs text-muted-foreground">
              <span class="font-medium text-foreground">{whenKept(chosen)}</span>
              · {causeOf(chosen)}
              {#if chosen.editors.length > 0}
                · {m.card_version_by({ who: whoWrote(chosen) })}
              {/if}
            </p>
            {#if reading.contentStatus === 'ready'}
              <VersionCompare
                {reading}
                {whenKept}
                {causeOf}
                {whoWrote}
                {resolveUser}
                {references}
                {words}
              />
            {:else}
              <div class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 py-3">
                {#if reading.contentStatus === 'loading'}
                  <p class="text-muted-foreground" role="status">{m.app_loading()}</p>
                {:else if reading.contentStatus === 'gone'}
                  <p role="alert">{m.card_version_gone()}</p>
                {:else}
                  <p role="alert">{m.card_version_read_failed()}</p>
                {/if}
              </div>
            {/if}
            {#if container.canEdit && reading.contentStatus === 'ready'}
              <div class="flex flex-wrap items-center justify-end gap-x-3 gap-y-2 border-t p-3">
                {#if reading.refusal}
                  <p class="min-w-0 flex-1 text-sm" role="alert" data-refusal={reading.refusal}>
                    {reading.refusal === 'gone'
                      ? m.card_version_gone()
                      : reading.refusal === 'unrestorable'
                        ? m.card_version_unrestorable()
                        : m.card_version_restore_failed()}
                  </p>
                {/if}
                {#if current}
                  <p class="text-sm text-muted-foreground" data-testid="body-version-is-current">
                    {words.isCurrent()}
                  </p>
                {:else}
                  <Button
                    class="roomy:min-h-touch"
                    disabled={reading.restoring}
                    data-testid="body-version-restore"
                    onclick={() => {
                      asking = true;
                    }}
                  >
                    <RotateCcwIcon aria-hidden="true" />
                    {m.card_version_restore()}
                  </Button>
                {/if}
              </div>
            {/if}
          {/if}
        </section>
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>

<ConfirmDialog
  bind:open={asking}
  title={m.card_version_restore_title()}
  description={chosen ? words.restoreDescription({ when: whenKept(chosen) }) : undefined}
  confirmLabel={m.card_version_restore_confirm()}
  onConfirm={() => void restore()}
/>
