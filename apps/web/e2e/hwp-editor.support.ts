/**
 * What the end-to-end tests of the Hangul editor share: a document attached,
 * shown and opened in the editor, typed into, and a version written by
 * somebody else meanwhile (the memory backend's other person,
 * `fvociDev.someoneElse`).
 */
import { expect, type Frame, type Locator, type Page } from '@playwright/test';

import { attach, viewer } from './file-viewer.support.ts';

export const HWPX_TYPE = 'application/hwp+zip';
export const HWP_TYPE = 'application/x-hwp';
/** What the tests type into a document. */
export const TYPED = 'EDITED IN FVOCI';

export const editor = (page: Page): Locator => page.getByTestId('file-editor');
export const said = (page: Page): Locator => page.getByTestId('file-editor-said');

interface Uploaded {
  fileId: string;
}

interface DevBackend {
  workspaces(): Promise<{ id: string }[]>;
  boards(workspaceId: string): Promise<{ id: string; title: string }[]>;
  cards(boardId: string): Promise<{ id: string; title: string }[]>;
  me(): Promise<{ id: string }>;
  addMember(boardId: string, userId: string, role: string): Promise<unknown>;
  attachments(boardId: string, cardId: string): Promise<{ name: string; fileId: string }[]>;
  upload(boardId: string, file: File): Promise<Uploaded>;
  fileVersions: {
    add(boardId: string, fileId: string, baseVersion: number, upload: Uploaded): Promise<unknown>;
  };
}

interface Dev {
  backend: DevBackend;
  someoneElse(): DevBackend;
}

/** Somebody else, an editor of the board, writes the file's second version: the sample as it is. */
export async function writtenElsewhere(page: Page, name: string, document: Buffer): Promise<void> {
  await page.evaluate(
    async ({ name, bytes }) => {
      const dev = (window as unknown as { fvociDev: Dev }).fvociDev;
      const [workspace] = await dev.backend.workspaces();
      const boards = await dev.backend.boards(workspace?.id ?? '');
      const board = boards.find((each) => each.title.startsWith('Project schedule'));
      const cards = await dev.backend.cards(board?.id ?? '');
      const card = cards.find((each) => each.title === 'Design the screens');
      if (!board || !card) throw new Error('no card');
      const other = dev.someoneElse();
      await dev.backend.addMember(board.id, (await other.me()).id, 'editor');
      const attached = (await dev.backend.attachments(board.id, card.id)).find(
        (each) => each.name === name,
      );
      if (!attached) throw new Error('the file is not attached');
      const file = new File([Uint8Array.from(bytes)], name, { type: 'application/hwp+zip' });
      const theirs = await other.upload(board.id, file);
      await other.fileVersions.add(board.id, attached.fileId, 1, theirs);
    },
    { name, bytes: [...document] },
  );
}

/** The editor's frame, to run code in. */
export function editorFrame(page: Page): Frame {
  const inside = page.frames().find((each) => each.url().includes('/static/editor-frame.html'));
  if (!inside) throw new Error('no editor frame');
  return inside;
}

/** Attaches a document, shows it in the viewer and opens the editor on it. */
export async function edit(
  page: Page,
  name: string,
  buffer: Buffer,
  mimeType: string,
): Promise<Locator> {
  const row = await attach(page, name, buffer, mimeType);
  await row.getByRole('link', { name }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await page.getByTestId('file-viewer-edit').click();
  await expect(editor(page)).toHaveAttribute('data-status', 'ready');
  await expect(viewer(page)).toHaveCount(0);
  return row;
}

/** Types into the document where the editor opens it, at its start: the focus is there already. */
export async function type(page: Page, text: string): Promise<void> {
  await expect(page.getByTestId('file-editor-frame')).toBeFocused();
  await page.keyboard.type(text);
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
}
