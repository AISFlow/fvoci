/**
 * Saving a board, a wiki, a course or a diary from the sidebar, apart from
 * the list, which is long: as an archive (`.fvoci`), which an import makes
 * again whole, or as Markdown files (`.zip`), for another tool.
 */
import DownloadIcon from '@lucide/svelte/icons/download';
import FileTextIcon from '@lucide/svelte/icons/file-text';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { saveBlob } from '$lib/save.ts';
import type { Container } from '$lib/logic/wire.ts';
import type { Workspace } from '$lib/state/index.ts';
import { exportContainerMarkdown } from '$lib/state/workspace-sync.ts';

/** Saves an archive of the container, once it has all arrived. */
async function exportContainer(workspace: Workspace, containerId: string): Promise<void> {
  const archive = await workspace.exportContainer(containerId);
  if (archive) saveBlob(archive.bytes, archive.name);
}

/** Saves the container's Markdown files, once they have all arrived. */
async function exportMarkdown(workspace: Workspace, containerId: string): Promise<void> {
  const { transfers, backend, containers } = workspace;
  const zip = await exportContainerMarkdown(transfers, backend, containers, containerId);
  if (zip) saveBlob(zip.bytes, zip.name);
}

/**
 * The entries of a container's menu that save it, under a heading of what it
 * is: a guest's too. Markdown only where the backend writes it.
 */
export function exportActions(
  workspace: Workspace,
  container: Pick<Container, 'id' | 'kind'>,
  separated = false,
): MenuAction[] {
  const containerId = container.id;
  const heading = {
    board: m.shell_export_board,
    wiki: m.shell_export_wiki,
    diary: m.shell_export_diary,
    course: m.shell_export_course,
  }[container.kind ?? 'board']();
  const archive: MenuAction = {
    id: 'export',
    label: m.shell_export_fvoci(),
    heading,
    icon: DownloadIcon,
    separated,
    run: () => void exportContainer(workspace, containerId),
  };
  if (!workspace.backend.markdown) return [archive];
  return [
    archive,
    {
      id: 'export-markdown',
      label: m.shell_export_markdown(),
      icon: FileTextIcon,
      run: () => void exportMarkdown(workspace, containerId),
    },
  ];
}
