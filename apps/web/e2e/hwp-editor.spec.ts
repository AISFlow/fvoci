/**
 * The editor of Hangul documents, opened from the viewer on a card's
 * attachment: a document edited, saved as the file's new version and shown
 * with the edit when opened again; an edit of a version that is no longer
 * the newest kept as a file of its own; and the frame the editor runs in
 * kept off the network, the app's storage and the app's page.
 */
import { expect, test } from '@playwright/test';

import {
  HWP_TEXT,
  koreanHwp,
  koreanHwpx,
  koreanHwpxWithOtherParts,
  rewriteArchive,
} from '../tests/support/hwp-files.ts';

import { attach, frameOf, openCard, viewer } from './file-viewer.support.ts';
import {
  edit,
  editor,
  editorFrame,
  HWP_TYPE,
  HWPX_TYPE,
  said,
  type,
  TYPED,
  writtenElsewhere,
} from './hwp-editor.support.ts';

for (const [name, sample, mimeType] of [
  ['회의록.hwpx', koreanHwpx, HWPX_TYPE],
  ['회의록.hwp', koreanHwp, HWP_TYPE],
] as const) {
  test(`${name} is edited, saved as a new version and shows the edit when opened again`, async ({
    page,
  }) => {
    await openCard(page);
    // The editor and the engine it drives are fetched when a document is first edited.
    const fetched: string[] = [];
    page.on('request', (request) => fetched.push(new URL(request.url()).pathname));
    const row = await attach(page, name, sample(), mimeType);
    await row.getByRole('link', { name }).click();
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    expect(fetched.filter((file) => /editor-frame|FileEditor/.test(file))).toEqual([]);
    await page.getByTestId('file-viewer-edit').click();
    await expect(editor(page)).toHaveAttribute('data-status', 'ready');
    expect(fetched.some((file) => file.endsWith('/static/editor-frame.html'))).toBe(true);

    // Nothing to save yet; what is typed is.
    await expect(editor(page)).toHaveAttribute('data-dirty', 'false');
    await type(page, TYPED);
    await expect(said(page)).toHaveText('There are unsaved changes');

    await page.getByTestId('file-editor-save').click();
    await expect(said(page)).toHaveText('Saved as version 2.');
    await expect(editor(page)).toHaveAttribute('data-dirty', 'false');
    await expect(page.getByTestId('file-editor-losses')).toHaveCount(0);

    // The focus is back in the document: typed again and saved with the key, confirmed in the parent, the next version.
    await expect(page.getByTestId('file-editor-frame')).toBeFocused();
    await page.keyboard.type('!');
    await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
    await page.keyboard.press('Control+s');
    await page.getByTestId('file-editor-confirm-version').click();
    await expect(said(page)).toHaveText('Saved as version 3.');

    // Closed, the viewer shows the file as it is now: the newest version, with the edit.
    await page.getByTestId('file-editor-close').click();
    await expect(editor(page)).toHaveCount(0);
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    const text = frameOf(page).locator('.page[data-page="1"] .hwp-text');
    await expect(text).toContainText(`${TYPED}!`);
    await expect(text).toContainText(HWP_TEXT.title);
    await expect(page.getByTestId('file-viewer-older')).toHaveCount(0);
    await page.getByTestId('file-viewer-versions').click();
    await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(3);

    // An earlier version is shown, not edited: an edit of one could not become a version.
    await expect(page.getByTestId('file-viewer-edit')).toHaveCount(1);
    await page
      .getByTestId('file-versions-list')
      .locator('li[data-version="1"]')
      .getByTestId('file-version-view')
      .click();
    await expect(page.getByTestId('file-viewer-older')).toHaveCount(1);
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    await expect(page.getByTestId('file-viewer-edit')).toHaveCount(0);

    // And so does the card, opened again.
    // Narrow, the list closed when the version was shown.
    const list = page.getByTestId('file-versions');
    if (await list.isVisible()) await page.getByTestId('file-versions-close').click();
    await expect(list).toHaveCount(0);
    await page.getByTestId('file-viewer-close').click();
    await expect(viewer(page)).toHaveCount(0);
    await row.getByRole('link', { name }).click();
    await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
    await expect(frameOf(page).locator('.page[data-page="1"] .hwp-text')).toContainText(TYPED);
  });
}

test('an edit of a version that is no longer the newest is kept as a file of its own', async ({
  page,
}) => {
  await openCard(page);
  const name = '안건.hwpx';
  await edit(page, name, koreanHwpx(), HWPX_TYPE);
  await type(page, TYPED);
  // Somebody else writes the file's second version while this one is being edited.
  await writtenElsewhere(page, name, koreanHwpx());

  await page.getByTestId('file-editor-save').click();
  const stale = page.getByTestId('file-editor-stale');
  await expect(stale).toContainText('The file got a newer version meanwhile');
  // Nothing was written over theirs, and nothing is saved yet.
  await expect(editor(page)).toHaveAttribute('data-dirty', 'true');
  await page.getByTestId('file-editor-stale-apart').click();
  await expect(stale).toHaveCount(0);
  await expect(said(page)).toHaveText('Saved as a file of its own: 안건 (edited).hwpx');
  await expect(editor(page)).toHaveAttribute('data-dirty', 'false');

  // Closed without being asked about changes: there are none that are not kept.
  await page.getByTestId('file-editor-close').click();
  await expect(editor(page)).toHaveCount(0);
  // The file has their version, the newest, without the edit, and no third.
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  const text = frameOf(page).locator('.page[data-page="1"] .hwp-text');
  await expect(text).toContainText(HWP_TEXT.title);
  await expect(text).not.toContainText(TYPED);
  await page.getByTestId('file-viewer-versions').click();
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(2);
  await page.keyboard.press('Escape');
  if (await viewer(page).isVisible()) await page.keyboard.press('Escape');
  await expect(viewer(page)).toHaveCount(0);

  // The edit is attached to the same card under the other name, and opens with what was typed.
  const attachments = page.getByTestId('card-attachments').locator('[data-attachment]');
  await expect(attachments.filter({ hasText: '안건' })).toHaveCount(2);
  await attachments.getByRole('link', { name: '안건 (edited).hwpx' }).click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(text).toContainText(TYPED);
});

test('a save that loses part of the document says what, and saves only what the writer chose', async ({
  page,
}) => {
  await openCard(page);
  // The sample with its picture's part gone from the archive: rhwp has nothing to write for it,
  // and says so in its report of the save.
  const name = '그림 없음.hwpx';
  const withoutPicture = rewriteArchive(koreanHwpx(), (member) =>
    member.name.startsWith('BinData/') ? undefined : member,
  );
  const row = await edit(page, name, withoutPicture, HWPX_TYPE);
  await type(page, TYPED);
  const uploads: string[] = [];
  page.on('request', (request) => {
    if (request.method() !== 'GET') uploads.push(new URL(request.url()).pathname);
  });

  await page.getByTestId('file-editor-save').click();
  const losses = page.getByTestId('file-editor-losses');
  await expect(losses).toContainText('Saving loses part of the document');
  await expect(losses).toContainText('A picture or other embedded data is saved empty');
  await expect(losses).toContainText('BinData/image1.png');
  // Left unchosen, nothing is saved and the document stays changed.
  await losses.getByRole('button', { name: 'Cancel' }).click();
  await expect(losses).toHaveCount(0);
  await expect(said(page)).toHaveText('There are unsaved changes');
  expect(uploads).toEqual([]);

  // Asked again and saved as a version all the same.
  await page.keyboard.press('Control+s');
  await expect(losses).toBeVisible();
  await page.getByTestId('file-editor-losses-version').click();
  await expect(said(page)).toHaveText('Saved as version 2.');

  // And once more, under another name: the file keeps its two versions.
  await type(page, '!');
  await page.getByTestId('file-editor-save').click();
  await page.getByTestId('file-editor-losses-apart').click();
  await expect(said(page)).toHaveText('Saved as a file of its own: 그림 없음 (edited).hwpx');
  await page.getByTestId('file-editor-close').click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await page.getByTestId('file-viewer-versions').click();
  await expect(page.getByTestId('file-versions-list').locator('li')).toHaveCount(2);
  await expect(row).toBeAttached();
});

test('a save that leaves out parts of the file rhwp does not keep names them, and asks first', async ({
  page,
}) => {
  await openCard(page);
  // Parts another program wrote: rhwp reads none of them, and its report of the save is empty.
  await edit(page, '다른 부분.hwpx', koreanHwpxWithOtherParts(), HWPX_TYPE);
  await type(page, TYPED);
  await page.getByTestId('file-editor-save').click();
  const losses = page.getByTestId('file-editor-losses');
  await expect(losses).toContainText('what is in the 2 places below');
  const listed = losses.locator('li');
  await expect(listed).toHaveText([
    /^A part of the file is left out: the editor keeps only what it reads\s*Contents\/history\.xml$/,
    /^A part of the file is left out: the editor keeps only what it reads\s*Custom\/notes\.xml$/,
  ]);
  await losses.getByRole('button', { name: 'Cancel' }).click();
  await expect(losses).toHaveCount(0);
  await expect(said(page)).toHaveText('There are unsaved changes');
});

test('the frame the editor runs in reaches no network, no storage and nothing of the app', async ({
  page,
}) => {
  await openCard(page);
  // A cookie and stored values of the app's, which the frame must not see.
  await page.evaluate(() => {
    document.cookie = 'fvoci_probe=1; path=/';
    localStorage.setItem('fvoci-probe', '1');
  });
  await edit(page, 'probe.hwpx', koreanHwpx(), HWPX_TYPE);
  const inside = editorFrame(page);
  const fromFrame: string[] = [];
  page.on('request', (request) => {
    if (request.frame() === inside) fromFrame.push(request.url());
  });

  const attempt = async (address: string): Promise<string> => {
    try {
      return `reached: ${String((await fetch(address)).status)}`;
    } catch (error) {
      return `refused: ${error instanceof Error ? error.name : String(error)}`;
    }
  };
  // A file of the app that any origin may read: only the frame's policy stops it.
  const open = inside.url();
  expect(await page.evaluate(attempt, open)).toBe('reached: 200');
  expect(await inside.evaluate(attempt, open)).toMatch(/^refused/);

  const found = await inside.evaluate(async (address) => {
    const tried = (act: () => unknown): string => {
      try {
        return `got: ${String(act())}`;
      } catch (error) {
        return `refused: ${error instanceof Error ? error.name : String(error)}`;
      }
    };
    // The frame has none of the browser's storage (its origin is none); the editor is given a
    // store in memory in its place.
    const kind = (name: 'localStorage' | 'sessionStorage'): string =>
      window[name] instanceof Storage ? 'of the browser' : 'in memory';
    const database = await new Promise<string>((resolve) => {
      try {
        const asked = indexedDB.open('fvoci-probe');
        asked.onsuccess = () => {
          resolve('opened');
        };
        asked.onerror = () => {
          resolve('refused');
        };
      } catch (error) {
        resolve(`refused: ${error instanceof Error ? error.name : String(error)}`);
      }
    });
    // A worker of the app's address: refused as it is made (Chromium), or not loaded (WebKit,
    // which makes the worker and fails its load: an error that is no error of a script run).
    const worker = await new Promise<string>((resolve) => {
      addEventListener('securitypolicyviolation', (event) => {
        if (event.effectiveDirective === 'worker-src') resolve('refused: worker-src');
      });
      try {
        const made = new Worker(address);
        made.onerror = (event) => {
          resolve(
            event instanceof ErrorEvent && event.message ? 'got: a run' : 'refused: not loaded',
          );
        };
      } catch (error) {
        resolve(`refused: ${error instanceof Error ? error.name : String(error)}`);
        return;
      }
      setTimeout(() => {
        resolve('got: a worker');
      }, 2000);
    });
    return {
      origin: self.origin,
      cookie: tried(() => document.cookie),
      localStorage: kind('localStorage'),
      sessionStorage: kind('sessionStorage'),
      // What the editor sees as its storage holds nothing of the app's.
      seen: tried(() => localStorage.getItem('fvoci-probe')),
      database,
      parent: tried(() => window.parent.document.title),
      parentStorage: tried(() => window.parent.localStorage.getItem('fvoci-probe')),
      top: tried(() => window.top?.location.href),
      worker,
      opened: tried(() => window.open('about:blank')),
    };
  }, open);
  expect(found.database).toMatch(/^refused/);
  expect(found.worker).toMatch(/^refused: (SecurityError|worker-src|not loaded)$/);
  expect({ ...found, database: '', worker: '' }).toEqual({
    origin: 'null',
    cookie: 'refused: SecurityError',
    localStorage: 'in memory',
    sessionStorage: 'in memory',
    seen: 'got: null',
    database: '',
    parent: 'refused: SecurityError',
    parentStorage: 'refused: SecurityError',
    top: 'refused: SecurityError',
    worker: '',
    opened: 'got: null',
  });

  // Edited and saved, the frame asked the network for nothing: the app stored the document.
  await type(page, TYPED);
  await page.getByTestId('file-editor-save').click();
  await expect(said(page)).toHaveText('Saved as version 2.');
  expect(fromFrame.filter((address) => !/^(blob|data):/.test(address))).toEqual([]);
});

test('the keyboard leaves the document for Save and Close with F6', async ({ page }) => {
  await openCard(page);
  await edit(page, '안건.hwpx', koreanHwpx(), HWPX_TYPE);
  await type(page, TYPED);
  // The editor keeps Tab and Escape for the document; F6 goes out, to the first control.
  await page.keyboard.press('F6');
  await expect(page.getByTestId('file-editor-save')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(said(page)).toHaveText('Saved as version 2.');
  // Saved, what is typed goes to the document again; out once more, on to Close.
  await expect(page.getByTestId('file-editor-frame')).toBeFocused();
  await page.keyboard.press('F6');
  await expect(page.getByTestId('file-editor-save')).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(page.getByTestId('file-editor-close')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(editor(page)).toHaveCount(0);
});

test('no other file is opened in the place of the document being edited', async ({ page }) => {
  await openCard(page);
  const name = '안건.hwpx';
  const row = await edit(page, name, koreanHwpx(), HWPX_TYPE);
  const inside = editorFrame(page);
  // The editor's own "open a file", which a link to another file in a document leads to: a
  // file nobody checked would be saved as this file's next version.
  await inside.locator('#file-input').setInputFiles({
    name: '다른 문서.hwp',
    mimeType: HWP_TYPE,
    buffer: koreanHwp(),
  });
  // Said in the editor's status line, which the smaller screens do not show.
  await expect(inside.getByText(/HOST_OWNS_DOCUMENT/).first()).toBeAttached();

  // The document is still the one the app gave: edited and saved, it is the file's next version.
  await page.getByTestId('file-editor-frame').focus();
  await type(page, TYPED);
  await page.getByTestId('file-editor-save').click();
  await expect(said(page)).toHaveText('Saved as version 2.');
  await page.getByTestId('file-editor-close').click();
  await expect(viewer(page)).toHaveAttribute('data-status', 'shown');
  await expect(frameOf(page).locator('.page[data-page="1"] .hwp-text')).toContainText(TYPED);
  await expect(row).toContainText(name);
});

test('the editor’s page, opened as a page of the app, runs nothing', async ({ page }) => {
  // Its own parent when it is nobody's frame: what it would say to the app, it would say here.
  // (The server sends every page under /static/ sandboxed, so it has no origin here either.)
  await page.addInitScript(() => {
    const heard: unknown[] = [];
    Object.assign(window, { heard });
    window.addEventListener('message', (event) => heard.push(event.data));
  });
  await page.goto('/static/editor-frame.html?chrome=embed');
  await page.waitForLoadState('networkidle');
  // Asked to open a document as the app asks, by the only sender it would hear.
  await page.evaluate(() => {
    window.postMessage(
      {
        kind: 'open',
        format: 'hwpx',
        name: 'a.hwpx',
        bytes: new ArrayBuffer(8),
        engine: new ArrayBuffer(8),
        theme: 'light',
        lang: 'en',
      },
      '*',
    );
  });
  await page.evaluate(() => new Promise((resolve) => setTimeout(resolve, 500)));
  const found = await page.evaluate(() => ({
    origin: self.origin,
    // Not even "ready": it does not listen.
    said: (window as unknown as { heard: { kind?: string }[] }).heard
      .map((message) => message.kind)
      .filter((kind) => kind !== 'open'),
    studio: 'rhwpStudio' in window,
    body: document.body.childElementCount,
  }));
  expect(found).toEqual({ origin: 'null', said: [], studio: false, body: 0 });
});
