/**
 * Real-server end-to-end tests for course-t5 (separate from `course.spec.ts`):
 * 1. Unit comments CRUD, live updates, edit history, undo delete, and unjoined read-only access
 * 2. Unit body versions (⌘S/Ctrl+S manual keep, diff compare, restore across reload)
 * 3. `[[` unit references and backlinks
 * 4. Public course reader (`/shared/<course>?r=<token>`): anonymous access, same-course
 *    reference navigation, outside reference stripping, no user identity leak, link toggle off -> 404,
 *    and own-space course public link
 * 5. Unit mention / comment notification bell click -> navigates to unit & reveals comment,
 *    plus unit watch toggle persistence
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { focusAtEnd } from '../e2e/embeds.support.ts';
import { primaryKey } from '../e2e/keyboard.support.ts';
import { pressInBar } from '../e2e/shell.support.ts';

import {
  makeBoard,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  uniqueName,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

function coursePath(workspaceId: string, courseId: string, unitId?: string): string {
  return `/w/${workspaceId}/courses/${courseId}${unitId ? `/${unitId}` : ''}`;
}

function sharedPath(courseId: string, token: string, unitId?: string): string {
  const parts = [courseId, unitId].filter((part) => part !== undefined);
  return `/shared/${parts.map(encodeURIComponent).join('/')}?r=${encodeURIComponent(token)}`;
}

function strayRequestsOf(page: Page, courseId: string): string[] {
  const stray: string[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (!url.pathname.startsWith('/api/')) return;
    const said = `${request.method()} ${url.pathname}`;
    if (request.method() !== 'GET' || !url.pathname.startsWith(`/api/v1/shared/${courseId}`)) {
      stray.push(said);
    }
    void request.allHeaders().then((headers) => {
      if (headers.cookie !== undefined) stray.push(`${said} with a cookie`);
    });
  });
  page.on('websocket', (socket) => {
    socket.on('framesent', ({ payload }) => {
      const command = JSON.parse(String(payload)) as Record<string, unknown>;
      if (
        command.type !== 'setPresence' ||
        !command.readToken ||
        (command.containerId !== '' && command.containerId !== courseId) ||
        command.workspaceId !== ''
      ) {
        stray.push(`unexpected socket command ${String(command.type)}`);
      }
    });
    socket.on('framereceived', () => stray.push('a public reader received a socket message'));
  });
  return stray;
}

const comments = (page: Page): Locator => page.getByTestId('card-comments');
const commentRows = (page: Page): Locator => comments(page).locator('[data-comment]');
const composer = (page: Page): Locator => page.getByTestId('comment-composer');
const versions = (page: Page): Locator => page.getByTestId('body-versions');
const versionRows = (page: Page): Locator => versions(page).locator('[data-version]');

test('unit comments, body versions (⌘S + restore), and [[ unit references on the real server', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('course-owner');
  const learner = person('course-learner');
  await register(base, owner);
  await register(base, learner);
  const api = await signedIn(base, owner);
  const { workspaceId } = await makeBoard(api, { title: 'Anchor board' });
  const at = `/api/v1/w/${workspaceId}`;

  const course = (await (
    await api.post(`${at}/courses`, { data: { title: 'Linear Algebra', visibility: 'open' } })
  ).json()) as Schemas['Container'];
  const chapter = (await (
    await api.post(`${at}/courses/${course.id}/units`, {
      data: { kind: 'chapter', title: 'Vectors' },
    })
  ).json()) as Schemas['Unit'];
  const unit1Title = uniqueName('Basis Vectors');
  const unit2Title = uniqueName('Inner Products');
  const unit1 = (await (
    await api.post(`${at}/courses/${course.id}/units`, {
      data: { kind: 'unit', title: unit1Title, parentId: chapter.id },
    })
  ).json()) as Schemas['Unit'];
  const unit2 = (await (
    await api.post(`${at}/courses/${course.id}/units`, {
      data: { kind: 'unit', title: unit2Title, parentId: chapter.id },
    })
  ).json()) as Schemas['Unit'];

  const ownerCtx = await browser.newContext({ locale: 'en-US', baseURL: base });
  const learnerCtx = await browser.newContext({ locale: 'en-US', baseURL: base });
  const ownerPage = await ownerCtx.newPage();
  const learnerPage = await learnerCtx.newPage();
  const errors = [watchErrors(ownerPage), watchErrors(learnerPage)];

  // 1. Open course: learner reads comments read-only before joining, then joins & comments live.
  await ownerPage.goto(coursePath(workspaceId, course.id, unit1.id));
  await signInOnPage(ownerPage, owner);
  await expect(ownerPage.getByTestId('unit-body')).toHaveAttribute('data-editor', 'ready');

  await learnerPage.goto(coursePath(workspaceId, course.id, unit1.id));
  await signInOnPage(learnerPage, learner);
  await expect(learnerPage.getByTestId('course-join')).toBeVisible();
  await expect(composer(learnerPage)).toHaveCount(0);

  // Join the open course -> composer appears and comment arrives live on owner's screen.
  await learnerPage.getByRole('button', { name: 'Enroll' }).click();
  await expect(composer(learnerPage)).toBeVisible();
  await composer(learnerPage).getByTestId('comment-field').fill('Clear explanation!');
  await composer(learnerPage).getByTestId('comment-send').click();
  await expect(commentRows(ownerPage)).toHaveCount(1);
  await expect(commentRows(ownerPage).first()).toContainText('Clear explanation!');

  // Edit comment, check history, delete and undo.
  const learnerRow = commentRows(learnerPage).first();
  await learnerRow.locator('[data-action="edit"]').click();
  const editField = learnerRow.getByRole('textbox', { name: 'Comment text' });
  await editField.fill('Very clear explanation!');
  await editField.press(await primaryKey(learnerPage, 'Enter'));
  await expect(commentRows(ownerPage).first()).toContainText('Very clear explanation!');
  await learnerRow.locator('[data-action="history"]').click();
  await expect(learnerRow.getByTestId('comment-history')).toContainText('Clear explanation!');

  await learnerRow.getByRole('button', { name: /^Delete the comment from/ }).click();
  await expect(commentRows(ownerPage)).toHaveCount(0);
  await pressInBar(learnerPage, 'undo');
  await expect(commentRows(ownerPage)).toHaveCount(1);

  // 2. Unit body versions: type v1, press ⌘S/Ctrl+S, write v2, restore v1 and verify across reload.
  const body = ownerPage.getByTestId('unit-body');
  await focusAtEnd(body);
  await ownerPage.keyboard.type('First basis definition.');
  await ownerPage.keyboard.press(await primaryKey(ownerPage, 's'));
  await expect(body.getByTestId('body-kept')).toContainText(/Version kept/i);

  // Write a second body whole via API so we have a distinct newer version.
  const freshUnit1 = (await (
    await api.get(`${at}/courses/${course.id}/units/${unit1.id}`)
  ).json()) as Schemas['Unit'];
  const patched = await api.patch(`${at}/courses/${course.id}/units/${unit1.id}`, {
    data: {
      readAt: freshUnit1.updatedAt,
      body: {
        type: 'doc',
        content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Second draft body.' }] }],
      },
    },
  });
  expect(patched.ok(), await patched.text()).toBe(true);
  await expect(body.locator('[contenteditable]').first()).toContainText('Second draft body.');

  // Open versions dialog, pick the older draft, restore it, and reload to verify persistence.
  await ownerPage.getByTestId('unit-menu').click();
  await ownerPage.locator('[data-action="versions"]').click();
  await expect(versions(ownerPage)).toBeVisible();
  await expect(versionRows(ownerPage)).toHaveCount(2);
  await versionRows(ownerPage).last().click();
  await expect(versions(ownerPage).getByTestId('body-version')).toContainText(
    'First basis definition.',
  );
  await ownerPage.getByTestId('body-version-restore').click();
  await ownerPage.getByTestId('confirm-dialog').getByRole('button', { name: 'Restore' }).click();
  await expect(versions(ownerPage)).toHaveCount(0);
  await expect(body.locator('[contenteditable]').first()).toContainText('First basis definition.');
  await ownerPage.reload();
  await expect(
    ownerPage.getByTestId('unit-body').locator('[contenteditable]').first(),
  ).toContainText('First basis definition.');

  // 3. `[[` unit reference and backlinks.
  await focusAtEnd(ownerPage.getByTestId('unit-body'));
  await ownerPage.keyboard.press('Enter');
  await ownerPage.keyboard.type(`Next: [[${unit2Title}`);
  const option = ownerPage
    .locator('[data-suggestions="reference"]')
    .getByRole('option', { name: `${unit2Title} · Unit` });
  await expect(option).toHaveCount(1);
  await ownerPage.keyboard.press('Enter');
  const chip = ownerPage
    .getByTestId('unit-body')
    .locator(`[data-reference][data-block-id="${unit2.id}"]`);
  await expect(chip).toHaveAttribute('data-state', 'shown');
  await expect(chip).toHaveAttribute('data-kind', 'unit');

  // Wait for room flush before checking backlinks on unit 2.
  await expect
    .poll(async () => {
      const refs = (await (
        await api.get(`${at}/references/to/${unit2.id}`)
      ).json()) as Schemas['LinkedBlock'][];
      return refs.map((r) => r.id);
    })
    .toContain(unit1.id);

  await chip.getByRole('link').click();
  await expect(ownerPage).toHaveURL(new RegExp(`/courses/${course.id}/${unit2.id}$`));
  await expect(ownerPage.getByTestId('card-backlinks').getByRole('link')).toHaveText([unit1Title]);

  expect(errors.flat()).toEqual([]);
  await ownerCtx.close();
  await learnerCtx.close();
  await api.dispose();
});

test('public course link reader strips outside references & user identities, and turns off cleanly', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const owner = person('course-pub');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const secretTitle = uniqueName('Private Board Card');
  const board = await makeBoard(api, { title: 'Private Board', cards: [{ title: secretTitle }] });
  const secretCardId = board.cards[secretTitle] ?? '';
  const workspaceId = board.workspaceId;
  const at = `/api/v1/w/${workspaceId}`;
  const me = (await (await api.get('/api/v1/users/me')).json()) as Schemas['User'];

  const course = (await (
    await api.post(`${at}/courses`, { data: { title: 'Open Syllabus', visibility: 'private' } })
  ).json()) as Schemas['Container'];
  const chapter = (await (
    await api.post(`${at}/courses/${course.id}/units`, {
      data: { kind: 'chapter', title: 'Module 1' },
    })
  ).json()) as Schemas['Unit'];
  const u1 = (await (
    await api.post(`${at}/courses/${course.id}/units`, {
      data: { kind: 'unit', title: 'Overview', parentId: chapter.id },
    })
  ).json()) as Schemas['Unit'];
  const u2 = (await (
    await api.post(`${at}/courses/${course.id}/units`, {
      data: { kind: 'unit', title: 'Practice', parentId: chapter.id },
    })
  ).json()) as Schemas['Unit'];

  // Body has: same-course unit ref, outside board card ref, and @mention of owner.
  const patched = await api.patch(`${at}/courses/${course.id}/units/${u1.id}`, {
    data: {
      readAt: u1.updatedAt,
      body: {
        type: 'doc',
        content: [
          {
            type: 'paragraph',
            content: [
              { type: 'text', text: 'Continue with ' },
              { type: 'reference', attrs: { id: u2.id } },
              { type: 'text', text: ' and private ' },
              { type: 'reference', attrs: { id: secretCardId } },
              { type: 'text', text: ' by ' },
              { type: 'mention', attrs: { userId: me.id } },
            ],
          },
        ],
      },
    },
  });
  expect(patched.ok(), await patched.text()).toBe(true);

  // Add a comment on u1 to prove comments never leak to public course readers.
  const commented = await api.post(`${at}/courses/${course.id}/units/${u1.id}/comments`, {
    data: {
      body: {
        type: 'doc',
        content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Internal comment' }] }],
      },
    },
  });
  expect(commented.ok()).toBe(true);

  // Enable public link from the UI Share dialog.
  const ownerCtx = await browser.newContext({ locale: 'en-US', baseURL: base });
  const ownerPage = await ownerCtx.newPage();
  const ownerErrors = watchErrors(ownerPage);
  await ownerPage.goto(coursePath(workspaceId, course.id, u1.id));
  await signInOnPage(ownerPage, owner);
  await ownerPage.getByTestId('share-course').click();
  const shareDialog = ownerPage.getByTestId('share-dialog');
  await expect(shareDialog).toBeVisible();
  await shareDialog.getByTestId('share-public').click();
  const linkInput = shareDialog.getByTestId('share-link');
  await expect(linkInput).toHaveValue(new RegExp(`/shared/${course.id}\\?r=[^&]+$`));
  const sharedUrl = await linkInput.inputValue();

  // Anonymous reader opens the shared link.
  const anonCtx = await browser.newContext({ locale: 'en-US', baseURL: base });
  const anonPage = await anonCtx.newPage();
  const anonErrors = watchErrors(anonPage);
  const stray = strayRequestsOf(anonPage, course.id);
  await anonPage.goto(sharedUrl);

  const reader = anonPage.getByTestId('shared-course');
  await expect(reader).toBeVisible();
  await expect(anonPage.getByTestId('shared-title')).toHaveText('Open Syllabus');
  await expect(reader.getByTestId('unit-title')).toHaveText('Overview');
  await expect(anonPage).toHaveURL(new RegExp(`/shared/${course.id}/${u1.id}\\?r=`));
  await expect(reader.getByTestId('shared-unit-tree').locator('[data-kind="chapter"]')).toHaveText(
    'Module 1',
  );
  await expect(reader.getByTestId('shared-unit-tree').getByRole('link')).toHaveText([
    'Overview',
    'Practice',
  ]);

  // Privacy & isolation assertions: no owner username, no secret card title, no internal comment, no edit controls.
  await expect(anonPage.getByText(owner.username)).toHaveCount(0);
  await expect(anonPage.getByText(secretTitle)).toHaveCount(0);
  await expect(anonPage.getByText('Internal comment')).toHaveCount(0);
  await expect(anonPage.getByRole('textbox')).toHaveCount(0);
  await expect(anonPage.getByTestId('sidebar')).toHaveCount(0);
  await expect(anonPage.getByTestId('unit-menu')).toHaveCount(0);
  await expect(anonPage.getByTestId('unit-watch-toggle')).toHaveCount(0);

  // Outside reference is stripped to unseen; same-course reference opens Practice.
  await expect(
    reader.getByTestId('unit-body').locator('[data-reference][data-state="unseen"]'),
  ).toHaveCount(1);
  await reader.getByTestId('unit-body').getByRole('link', { name: 'Practice' }).click();
  await expect(anonPage).toHaveURL(new RegExp(`/shared/${course.id}/${u2.id}\\?r=`));
  await expect(reader.getByTestId('unit-title')).toHaveText('Practice');
  expect(stray).toEqual([]);

  // Turn off public link -> reload shows not-found.
  await shareDialog.getByTestId('share-public').click();
  await expect(shareDialog.getByTestId('share-link')).toHaveCount(0);
  await anonPage.reload();
  await expect(anonPage.getByTestId('not-found')).toBeVisible();

  // Also verify own-space course supports public link toggle.
  const workspaces = (await (
    await api.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const ownSpace = workspaces.find((ws) => ws.kind === 'personal');
  expect(ownSpace).toBeTruthy();
  const ownCourse = (await (
    await api.post(`/api/v1/w/${ownSpace?.id}/courses`, {
      data: { title: 'Personal Study', visibility: 'private' },
    })
  ).json()) as Schemas['Container'];
  const ownUnit = (await (
    await api.post(`/api/v1/w/${ownSpace?.id}/courses/${ownCourse.id}/units`, {
      data: { kind: 'unit', title: 'Notes' },
    })
  ).json()) as Schemas['Unit'];
  const ownShare = (await (
    await api.post(`/api/v1/w/${ownSpace?.id}/containers/${ownCourse.id}/public-link`, {
      data: { enabled: true },
    })
  ).json()) as Schemas['PublicLink'];
  await anonPage.goto(sharedPath(ownCourse.id, ownShare.token ?? '', ownUnit.id));
  await expect(anonPage.getByTestId('shared-title')).toHaveText('Personal Study');
  await expect(anonPage.getByTestId('shared-course').getByTestId('unit-title')).toHaveText('Notes');

  expect([...ownerErrors, ...anonErrors]).toEqual([]);
  await ownerCtx.close();
  await anonCtx.close();
  await api.dispose();
});

test('unit mention and comment notifications navigate to the unit and watch toggle persists', async ({
  browser,
  baseURL,
}) => {
  const base = baseURL ?? '';
  const writer = person('unit-writer');
  const reader = person('unit-reader');
  await register(base, writer);
  await register(base, reader);
  const writerApi = await signedIn(base, writer);
  const readerApi = await signedIn(base, reader);
  const { workspaceId } = await makeBoard(writerApi, { title: 'Notify Anchor' });
  const at = `/api/v1/w/${workspaceId}`;
  const readerMe = (await (await readerApi.get('/api/v1/users/me')).json()) as Schemas['User'];

  const course = (await (
    await writerApi.post(`${at}/courses`, { data: { title: 'Notify Course', visibility: 'open' } })
  ).json()) as Schemas['Container'];
  const unit = (await (
    await writerApi.post(`${at}/courses/${course.id}/units`, {
      data: { kind: 'unit', title: 'Watched Lesson' },
    })
  ).json()) as Schemas['Unit'];

  // Reader opens the workspace on their welcome screen (not on the course yet).
  const readerCtx = await browser.newContext({ locale: 'en-US', baseURL: base });
  const readerPage = await readerCtx.newPage();
  const errors = watchErrors(readerPage);
  await readerPage.goto(`/w/${workspaceId}`);
  await signInOnPage(readerPage, reader);
  await expect(readerPage.getByTestId('open-notifications')).toBeVisible();

  // Writer posts a comment mentioning reader on the unit.
  const commentResp = await writerApi.post(`${at}/courses/${course.id}/units/${unit.id}/comments`, {
    data: {
      body: {
        type: 'doc',
        content: [
          {
            type: 'paragraph',
            content: [
              { type: 'text', text: 'Please review ' },
              { type: 'mention', attrs: { userId: readerMe.id } },
            ],
          },
        ],
      },
    },
  });
  expect(commentResp.ok(), await commentResp.text()).toBe(true);
  const posted = (await commentResp.json()) as Schemas['Block'];

  // Notification rings reader's bell; clicking it navigates straight to the unit & reveals comment.
  await expect(readerPage.getByTestId('unread-notifications')).toHaveText('1');
  await readerPage.getByTestId('open-notifications').click();
  const inbox = readerPage.getByTestId('notifications');
  await expect(inbox).toBeVisible();
  const item = inbox.getByRole('link', { name: /Watched Lesson/ });
  await expect(item).toHaveAttribute('href', coursePath(workspaceId, course.id, unit.id));
  await item.click();
  await expect(readerPage).toHaveURL(new RegExp(`/courses/${course.id}/${unit.id}$`));
  await expect(readerPage.locator(`[data-comment="${posted.id}"]`)).toBeVisible();

  // Watch toggle toggles subscription on the real server and persists across reload.
  const watch = readerPage.getByTestId('unit-watch-toggle');
  await expect(watch).toBeVisible();
  await expect(watch).toHaveAttribute('aria-pressed', 'false');
  await watch.click();
  await expect(watch).toHaveAttribute('aria-pressed', 'true');
  await readerPage.reload();
  await expect(readerPage.getByTestId('unit-watch-toggle')).toHaveAttribute('aria-pressed', 'true');
  await readerPage.getByTestId('unit-watch-toggle').click();
  await expect(readerPage.getByTestId('unit-watch-toggle')).toHaveAttribute(
    'aria-pressed',
    'false',
  );

  // Own-space course unit has no watch toggle.
  const readerWorkspaces = (await (
    await readerApi.get('/api/v1/users/me/workspaces')
  ).json()) as Schemas['UserWorkspace'][];
  const ownSpace = readerWorkspaces.find((ws) => ws.kind === 'personal');
  expect(ownSpace).toBeTruthy();
  const ownCourse = (await (
    await readerApi.post(`/api/v1/w/${ownSpace?.id}/courses`, {
      data: { title: 'My Private Course', visibility: 'private' },
    })
  ).json()) as Schemas['Container'];
  const ownUnit = (await (
    await readerApi.post(`/api/v1/w/${ownSpace?.id}/courses/${ownCourse.id}/units`, {
      data: { kind: 'unit', title: 'Private Unit' },
    })
  ).json()) as Schemas['Unit'];
  await readerPage.goto(coursePath(ownSpace?.id ?? '', ownCourse.id, ownUnit.id));
  await expect(readerPage.getByTestId('unit-title')).toHaveValue('Private Unit');
  await expect(readerPage.getByTestId('unit-watch-toggle')).toHaveCount(0);
  expect(errors).toEqual([]);
  await readerCtx.close();
  await writerApi.dispose();
  await readerApi.dispose();
});
