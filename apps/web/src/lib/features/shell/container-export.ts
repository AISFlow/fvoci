/** Saving a board or a wiki as an archive from the sidebar, apart from the list, which is long. */
import DownloadIcon from '@lucide/svelte/icons/download';

import type { MenuAction } from '$lib/components/menu/actions.ts';
import { m } from '$lib/i18n/paraglide/messages.js';
import { saveBlob } from '$lib/save.ts';
import type { Container } from '$lib/logic/wire.ts';
import type { Workspace } from '$lib/state/index.ts';

/** Saves an archive of the board or wiki, once it has all arrived. */
async function exportContainer(workspace: Workspace, containerId: string): Promise<void> {
  const archive = await workspace.exportContainer(containerId);
  if (archive) saveBlob(archive.bytes, archive.name);
}

/** The entry of a board's or a wiki's menu that saves it as an archive: a guest's too. */
export function exportAction(
  workspace: Workspace,
  container: Pick<Container, 'id' | 'kind'>,
  separated = false,
): MenuAction {
  const containerId = container.id;
  return {
    id: 'export',
    label: {
      board: m.shell_export_board,
      wiki: m.shell_export_wiki,
      diary: m.shell_export_diary,
      course: m.shell_export_course,
    }[container.kind ?? 'board'](),
    icon: DownloadIcon,
    separated,
    run: () => void exportContainer(workspace, containerId),
  };
}
