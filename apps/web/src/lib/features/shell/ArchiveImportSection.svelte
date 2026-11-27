<!--
  Archive export and multi-format import (`fvoci`, `trello`, `notion`) inside the
  workspace settings dialog. Shows inline upload progress while an import runs,
  then renders a structured result summary with created counts, skipped/warning
  details, members left out, and direct links to newly created boards or wikis.
-->
<script lang="ts">
  import DownloadIcon from '@lucide/svelte/icons/download';
  import UploadIcon from '@lucide/svelte/icons/upload';

  import { m } from '$lib/i18n/paraglide/messages.js';
  import type { ContainerPlace } from '$lib/logic/urls.ts';
  import type { Container } from '$lib/logic/wire.ts';
  import { saveBlob } from '$lib/save.ts';
  import {
    IMPORT_FORMATS,
    isGuest,
    type ImportFormat,
    type ImportSummary,
    type Workspace,
  } from '$lib/state/index.ts';
  import { Button } from '$lib/ui/button/index.js';

  import SettingChoice from './SettingChoice.svelte';

  interface Props {
    workspace: Workspace;
    workspaceName: string;
    /** Completed summary for the current workspace, cleared when the dialog closes. */
    summary?: ImportSummary | undefined;
    onStartImport: () => number;
    onImported: (workspaceId: string, summary: ImportSummary, session: number) => void;
    containerHref?: ((container: ContainerPlace) => string) | undefined;
    onOpenContainer?: ((container: ContainerPlace) => void) | undefined;
  }

  let {
    workspace,
    workspaceName,
    summary,
    onStartImport,
    onImported,
    containerHref,
    onOpenContainer,
  }: Props = $props();

  const uid = $props.id();
  let format = $state<ImportFormat>('fvoci');
  let importField: HTMLInputElement | null = null;
  function attachImportField(node: HTMLInputElement): () => void {
    importField = node;
    return () => {
      if (importField === node) importField = null;
    };
  }

  const ACCEPT_BY_FORMAT: Record<ImportFormat, string> = {
    fvoci: '.fvoci,.zip,application/zip',
    trello: '.json,application/json',
    notion: '.zip,application/zip',
  };

  const FORMAT_LABELS: Record<ImportFormat, () => string> = {
    fvoci: m.shell_import_format_fvoci,
    trello: m.shell_import_format_trello,
    notion: m.shell_import_format_notion,
  };

  const hintText = $derived.by(() => {
    switch (format) {
      case 'trello':
        return m.shell_import_hint_trello({ workspace: workspaceName });
      case 'notion':
        return m.shell_import_hint_notion({ workspace: workspaceName });
      default:
        return m.shell_import_hint({ workspace: workspaceName });
    }
  });

  const activeImport = $derived(
    workspace.transfers
      .of(workspace.info?.id)
      .find((transfer) => transfer.kind === 'import' && !transfer.finished),
  );

  const leftOutCount = $derived(summary?.membersLeftOut ?? 0);

  const createdContainers = $derived(
    (summary?.containerIds ?? [])
      .map((id) => workspace.containers.find((one) => one.id === id))
      .filter((one): one is Container => one !== undefined),
  );

  const countBadges = $derived.by(() => {
    const counts = summary?.imported;
    if (!counts) return [];
    const items: { key: string; label: string }[] = [];
    if (counts.boards > 0) {
      items.push({ key: 'boards', label: m.shell_import_count_boards({ count: counts.boards }) });
    }
    if (counts.wikis > 0) {
      items.push({ key: 'wikis', label: m.shell_import_count_wikis({ count: counts.wikis }) });
    }
    if ((counts.courses ?? 0) > 0) {
      items.push({
        key: 'courses',
        label: m.shell_import_count_courses({ count: counts.courses ?? 0 }),
      });
    }
    if (counts.cards > 0) {
      items.push({ key: 'cards', label: m.shell_import_count_cards({ count: counts.cards }) });
    }
    if (counts.pages > 0) {
      items.push({ key: 'pages', label: m.shell_import_count_pages({ count: counts.pages }) });
    }
    if ((counts.units ?? 0) > 0) {
      items.push({
        key: 'units',
        label: m.shell_import_count_units({ count: counts.units ?? 0 }),
      });
    }
    if (counts.comments > 0) {
      items.push({
        key: 'comments',
        label: m.shell_import_count_comments({ count: counts.comments }),
      });
    }
    if (counts.files > 0) {
      items.push({ key: 'files', label: m.shell_import_count_files({ count: counts.files }) });
    }
    return items;
  });

  const hasSummaryCard = $derived(
    summary !== undefined &&
      (countBadges.length > 0 ||
        createdContainers.length > 0 ||
        summary.skipped.length > 0 ||
        summary.warnings.length > 0 ||
        (summary.warningsOmitted ?? 0) > 0),
  );

  async function exportWorkspace(): Promise<void> {
    const archive = await workspace.exportWorkspace();
    if (archive) saveBlob(archive.bytes, archive.name);
  }

  function handleFileChange(event: Event & { currentTarget: HTMLInputElement }): void {
    const field = event.currentTarget;
    const file = field.files?.[0];
    // Clear value so the same file can be chosen again.
    field.value = '';
    const workspaceId = workspace.info?.id;
    if (!file || workspaceId === undefined) return;
    const chosenFormat = format;
    const session = onStartImport();
    void workspace.importArchive(file, chosenFormat).then((result) => {
      if (result) onImported(workspaceId, result, session);
    });
  }

  function openCreated(event: MouseEvent, container: Container): void {
    if (
      !onOpenContainer ||
      event.button !== 0 ||
      event.metaKey ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey
    ) {
      return;
    }
    event.preventDefault();
    onOpenContainer({
      id: container.id,
      workspaceId: container.workspaceId,
      kind: container.kind,
    });
  }
</script>

<section class="grid gap-3 border-t pt-4" aria-labelledby="{uid}-archives">
  <h4 id="{uid}-archives" class="font-medium">{m.shell_archive_title()}</h4>

  {#if !isGuest(workspace)}
    <SettingChoice
      name="importFormat"
      label={m.shell_import_format()}
      value={format}
      options={IMPORT_FORMATS.map((value) => ({ value, label: FORMAT_LABELS[value]() }))}
      onChange={(next: ImportFormat) => {
        format = next;
      }}
    />
  {/if}

  <div class="flex flex-wrap gap-2">
    <Button
      variant="outline"
      class="roomy:min-h-touch"
      data-testid="export-workspace"
      onclick={() => void exportWorkspace()}
    >
      <DownloadIcon aria-hidden="true" />
      {m.shell_export_workspace()}
    </Button>
    <!-- A guest brings no boards in: importing makes boards. -->
    {#if !isGuest(workspace)}
      <Button
        variant="outline"
        class="roomy:min-h-touch"
        aria-describedby="{uid}-import-hint"
        data-testid="import-archive-trigger"
        disabled={activeImport !== undefined}
        onclick={() => importField?.click()}
      >
        <UploadIcon aria-hidden="true" />
        {m.shell_import()}
      </Button>
      <input
        {@attach attachImportField}
        type="file"
        accept={ACCEPT_BY_FORMAT[format]}
        hidden
        data-testid="import-archive"
        onchange={handleFileChange}
      />
    {/if}
  </div>

  {#if !isGuest(workspace)}
    <p id="{uid}-import-hint" class="text-xs text-muted-foreground" data-testid="import-hint">
      {hintText}
    </p>
  {/if}

  {#if activeImport}
    <div
      class="grid gap-1 rounded-md border bg-muted/40 p-2.5 text-xs"
      data-testid="import-progress"
    >
      <div class="flex items-center justify-between gap-2">
        <span class="truncate font-medium">
          {m.shell_transfer_import({ name: activeImport.name })}
        </span>
        {#if activeImport.total > 0}
          <span class="shrink-0 text-muted-foreground tabular-nums">
            {Math.min(100, Math.round((activeImport.done / activeImport.total) * 100))}%
          </span>
        {/if}
      </div>
      <progress
        class="h-1.5 w-full accent-primary"
        max={activeImport.total || undefined}
        value={activeImport.total ? activeImport.done : undefined}
        aria-label={m.shell_transfer_import({ name: activeImport.name })}
      ></progress>
    </div>
  {/if}

  {#if hasSummaryCard && summary}
    <div
      class="grid gap-2.5 rounded-md border bg-muted/30 p-3 text-xs"
      data-testid="import-summary"
    >
      <div class="flex items-center justify-between gap-2">
        <span class="font-medium text-foreground">{m.shell_import_summary_title()}</span>
        <span class="rounded bg-muted px-1.5 py-0.5 text-[11px] text-muted-foreground uppercase">
          {summary.format}
        </span>
      </div>

      {#if countBadges.length > 0}
        <ul class="flex flex-wrap gap-1.5" data-testid="import-counts">
          {#each countBadges as badge (badge.key)}
            <li
              class="rounded bg-background px-2 py-0.5 font-medium text-foreground shadow-xs ring-1 ring-border"
              data-import-count={badge.key}
            >
              {badge.label}
            </li>
          {/each}
        </ul>
      {/if}

      {#if createdContainers.length > 0}
        <div class="grid gap-1" data-testid="import-created-links">
          <span class="text-muted-foreground">{m.shell_import_created_heading()}</span>
          <ul class="flex flex-wrap gap-1.5">
            {#each createdContainers as container (container.id)}
              {@const place = {
                id: container.id,
                workspaceId: container.workspaceId,
                kind: container.kind,
              }}
              <li>
                {#if containerHref}
                  <a
                    href={containerHref(place)}
                    class="inline-flex items-center rounded border bg-background px-2 py-1 font-medium text-primary underline-offset-2 hover:underline"
                    data-testid="import-created-link"
                    onclick={(event) => {
                      openCreated(event, container);
                    }}
                  >
                    {container.title}
                  </a>
                {:else if onOpenContainer}
                  <button
                    type="button"
                    class="inline-flex items-center rounded border bg-background px-2 py-1 font-medium text-primary underline-offset-2 hover:underline"
                    data-testid="import-created-link"
                    onclick={(event) => {
                      openCreated(event, container);
                    }}
                  >
                    {container.title}
                  </button>
                {:else}
                  <span
                    class="inline-flex items-center rounded border bg-background px-2 py-1 font-medium"
                  >
                    {container.title}
                  </span>
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if summary.skipped.length > 0}
        <details class="rounded border bg-background/80 p-2" data-testid="import-skipped">
          <summary class="cursor-pointer font-medium text-foreground">
            {m.shell_import_skipped_summary({ count: summary.skipped.length })}
          </summary>
          <ul class="mt-1.5 grid max-h-40 gap-1 overflow-y-auto text-muted-foreground">
            {#each summary.skipped as entry, index (`${index}:${entry.code}:${entry.item}`)}
              <li class="break-words">
                <span class="font-medium text-foreground">{entry.item}</span>: {entry.reason}
              </li>
            {/each}
          </ul>
        </details>
      {/if}

      {#if summary.warnings.length > 0}
        <details class="rounded border bg-background/80 p-2" data-testid="import-warnings">
          <summary class="cursor-pointer font-medium text-foreground">
            {m.shell_import_warnings_summary({ count: summary.warnings.length })}
          </summary>
          <ul class="mt-1.5 grid max-h-40 gap-1 overflow-y-auto text-muted-foreground">
            {#each summary.warnings as entry, index (`${index}:${entry.code}:${entry.item}`)}
              <li class="break-words">
                <span class="font-medium text-foreground">{entry.item}</span>: {entry.reason}
              </li>
            {/each}
          </ul>
        </details>
      {/if}

      {#if (summary.warningsOmitted ?? 0) > 0}
        <p class="text-muted-foreground" data-testid="import-warnings-omitted">
          {m.shell_import_warnings_omitted({ count: summary.warningsOmitted ?? 0 })}
        </p>
      {/if}
    </div>
  {/if}

  <!-- Present before it says anything, so that what it says is announced. -->
  <p role="status" class="sr-only" data-testid="import-status">
    {#if hasSummaryCard && summary}
      {m.shell_import_summary_title()}{#if countBadges.length > 0}: {countBadges
          .map((badge) => badge.label)
          .join(', ')}{/if}
    {/if}
  </p>
  <p role="status" class={leftOutCount > 0 ? 'text-xs' : 'sr-only'} data-testid="import-left-out">
    {#if leftOutCount > 0}{m.shell_import_left_out({
        count: leftOutCount,
        workspace: workspaceName,
      })}{/if}
  </p>
</section>
