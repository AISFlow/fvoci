<!--
  The versions kept of the file open in the viewer (FileViewer.svelte), the
  newest first: when each was written, by whom and how large it is. A version
  can be looked at in the viewer and downloaded. Who edits the board or wiki
  can go back to one, after being asked (its bytes become a new version, the
  ones between stay), and upload a new version from a file of the same kind.

  A write is made from the newest version the list had. When somebody else
  wrote one meanwhile nothing is changed, and the list says so; a file that
  went up to become a version can then be saved under another name, which
  attaches it as a file of its own.
-->
<script lang="ts">
  import DownloadIcon from '@lucide/svelte/icons/download';
  import EyeIcon from '@lucide/svelte/icons/eye';
  import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
  import UploadIcon from '@lucide/svelte/icons/upload';
  import XIcon from '@lucide/svelte/icons/x';

  import ConfirmDialog from '$lib/components/confirm/ConfirmDialog.svelte';
  import IconButton from '$lib/components/icon-button/IconButton.svelte';
  import type {
    FileVersion,
    VersionsList,
    VersionsSource,
  } from '$lib/file-viewer/versions.svelte.ts';
  import { m } from '$lib/i18n/paraglide/messages.js';
  import { getLocale } from '$lib/i18n/paraglide/runtime.js';
  import { displayDateTime, humanFileSize, relativeTime } from '$lib/logic/format.ts';
  import type { Regional } from '$lib/logic/regional.ts';
  import { regional as deviceRegional } from '$lib/regional.ts';
  import { Button } from '$lib/ui/button/index.js';

  interface Props {
    list: VersionsList;
    source: VersionsSource;
    /** The version the viewer shows, once known. */
    shown: number | undefined;
    /** The kind of file a new version is picked from: the file's extension, with its dot, or none. */
    accept: string;
    regional?: Regional | undefined;
    /** Shows a version in the viewer. */
    onView: (version: FileVersion) => void;
    onDownload: (version: FileVersion) => void;
    /** The file has a new newest version, written here. */
    onWritten: (version: FileVersion) => void;
    onClose: () => void;
  }

  let {
    list,
    source,
    shown,
    accept,
    regional: regionalProp,
    onView,
    onDownload,
    onWritten,
    onClose,
  }: Props = $props();

  const uid = $props.id();

  let input = $state<HTMLInputElement>();
  /** The version the reader is asked about going back to. */
  let asked = $state.raw<FileVersion>();
  let asking = $state(false);

  const context = $derived.by(() => {
    const locale = getLocale();
    const { timeZone, hourCycle } = regionalProp ?? deviceRegional(locale);
    return { locale, timeZone, hourCycle };
  });
  const busy = $derived(list.writing !== undefined);

  const when = (version: FileVersion): string => displayDateTime(version.createdAt, context);
  const who = (version: FileVersion): string =>
    source.userName(version.createdBy) ?? m['centerPanel.unknown-user']();

  /** What a refusal says. */
  const refusal = $derived.by((): string | undefined => {
    switch (list.refusal) {
      case 'stale':
        return list.apart ? m.file_version_stale_upload() : m.file_version_stale();
      case 'gone':
        return m.file_version_gone();
      case 'unusable':
        return m.file_version_unusable();
      case 'failed':
        return m.file_version_failed();
      default:
        return undefined;
    }
  });

  async function add(file: File | undefined): Promise<void> {
    if (!file) return;
    const written = await list.add(file);
    if (written) onWritten(written);
  }

  async function restore(): Promise<void> {
    const version = asked;
    if (!version) return;
    const written = await list.restore(version);
    if (written) onWritten(written);
  }
</script>

<aside
  class="flex min-h-0 min-w-0 flex-1 flex-col bg-background"
  aria-labelledby="{uid}-title"
  data-testid="file-versions"
>
  <div class="flex min-h-12 items-center gap-1 border-b py-1 pr-1 pl-4">
    <h2 id="{uid}-title" class="min-w-0 flex-1 truncate font-semibold outline-none" tabindex="-1">
      {m.file_versions_title()}
    </h2>
    <IconButton label={m.file_versions_close()} data-testid="file-versions-close" onclick={onClose}>
      <XIcon aria-hidden="true" />
    </IconButton>
  </div>

  {#if source.canWrite}
    <div class="border-b p-2">
      <Button
        variant="outline"
        class="w-full roomy:min-h-touch"
        disabled={busy || !list.current}
        data-testid="file-version-add"
        onclick={() => input?.click()}
      >
        <UploadIcon data-icon="inline-start" aria-hidden="true" />
        {m.file_version_add()}
      </Button>
      <input
        bind:this={input}
        type="file"
        accept={accept || undefined}
        class="sr-only"
        tabindex="-1"
        aria-hidden="true"
        data-testid="file-version-input"
        onchange={(event) => {
          const [file] = event.currentTarget.files ?? [];
          // The same file can be picked again.
          event.currentTarget.value = '';
          void add(file);
        }}
      />
    </div>
  {/if}

  <p class="sr-only" aria-live="polite" data-testid="file-versions-progress">
    {list.writing === 'upload'
      ? m.file_version_adding()
      : list.writing === 'restore'
        ? m.file_version_restoring()
        : ''}
  </p>

  {#if refusal}
    <div
      class="grid gap-2 border-b bg-muted p-3 text-sm"
      role="alert"
      data-testid="file-version-refusal"
      data-refusal={list.refusal}
    >
      <p>{refusal}</p>
      <div class="flex flex-wrap gap-2">
        {#if list.apart}
          <Button
            class="roomy:min-h-touch"
            disabled={busy}
            data-testid="file-version-save-apart"
            onclick={() => void list.attachApart()}
          >
            {m.file_version_save_apart()}
          </Button>
        {/if}
        <Button
          variant="outline"
          class="roomy:min-h-touch"
          disabled={busy}
          data-testid="file-version-dismiss"
          onclick={() => {
            list.dismiss();
          }}
        >
          {list.apart ? m.file_version_discard() : m.ui_close()}
        </Button>
      </div>
    </div>
  {/if}

  <div class="min-h-0 flex-1 overflow-y-auto overscroll-contain">
    {#if list.status === 'loading'}
      <p class="p-4 text-muted-foreground" role="status">{m.app_loading()}</p>
    {:else if list.status === 'failed'}
      <div class="grid justify-items-start gap-2 p-4">
        <p role="alert">{m.file_versions_failed()}</p>
        <Button variant="outline" class="roomy:min-h-touch" onclick={() => list.read()}>
          {m.app_retry()}
        </Button>
      </div>
    {:else}
      <ol class="grid gap-1 p-1.5" data-testid="file-versions-list">
        {#each list.versions as version (version.version)}
          {@const at = when(version)}
          <li
            class={['grid gap-1 rounded-md px-2.5 py-2', shown === version.version && 'bg-muted']}
            data-version={version.version}
            data-shown={shown === version.version ? '' : undefined}
          >
            <p class="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span class="font-medium" data-testid="file-version-number">
                {m.file_version_number({ version: version.version })}
              </span>
              <time
                class="text-muted-foreground"
                datetime={new Date(version.createdAt).toISOString()}
              >
                {relativeTime(version.createdAt, context.locale)}
              </time>
              {#if version.isCurrent}
                <span
                  class="rounded-sm bg-primary/10 px-1.5 py-0.5 text-xs font-medium text-primary"
                  data-testid="file-version-current"
                >
                  {m.file_version_current()}
                </span>
              {/if}
              {#if shown === version.version}
                <span class="rounded-sm border px-1.5 py-0.5 text-xs font-medium">
                  {m.file_version_shown()}
                </span>
              {/if}
            </p>
            <p class="text-xs text-muted-foreground">
              {at} · {humanFileSize(version.size)}
            </p>
            <p class="min-w-0 text-xs break-words text-muted-foreground">
              <span data-testid="file-version-by">{m.file_version_by({ who: who(version) })}</span>
              {#if version.restoredFrom !== null}
                · {m.file_version_restored_from({ version: version.restoredFrom })}
              {/if}
            </p>
            <div class="flex gap-1">
              <!-- Unavailable rather than disabled, here and below: a button disabled under the
                   focus drops it to the page, and the keyboard's place in the list with it. -->
              <IconButton
                label={m.file_version_view({ version: version.version, when: at })}
                aria-disabled={shown === version.version ? 'true' : undefined}
                class="aria-disabled:opacity-50"
                data-testid="file-version-view"
                onclick={() => {
                  if (shown !== version.version) onView(version);
                }}
              >
                <EyeIcon aria-hidden="true" />
              </IconButton>
              <IconButton
                label={m.file_version_download({ version: version.version, when: at })}
                data-testid="file-version-download"
                onclick={() => {
                  onDownload(version);
                }}
              >
                <DownloadIcon aria-hidden="true" />
              </IconButton>
              {#if source.canWrite && !version.isCurrent}
                <IconButton
                  label={m.file_version_restore({ version: version.version, when: at })}
                  aria-disabled={busy ? 'true' : undefined}
                  class="aria-disabled:opacity-50"
                  data-testid="file-version-restore"
                  onclick={() => {
                    if (busy) return;
                    asked = version;
                    asking = true;
                  }}
                >
                  <RotateCcwIcon aria-hidden="true" />
                </IconButton>
              {/if}
            </div>
          </li>
        {/each}
      </ol>
    {/if}
  </div>
</aside>

<ConfirmDialog
  bind:open={asking}
  title={m.file_version_restore_title()}
  description={asked ? m.file_version_restore_description({ when: when(asked) }) : undefined}
  confirmLabel={m.file_version_restore_confirm()}
  onConfirm={() => void restore()}
/>
