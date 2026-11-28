/**
 * Course L6 live end-to-end against the real Rust server:
 * - Creates a course from the built-in course template from the window of new things
 * - Verifies chapters, units, study amounts, and shared `course_schedules` on the server
 * - Records personal progress (`completed`, `lastPosition`, personal `plannedDate`)
 * - Duplicates the course via sidebar menu -> verifies amounts & shared schedule preserved,
 *   and zero personal progress copied
 * - Exports the course archive and imports it into a second workspace -> verifies units,
 *   amounts, and shared schedule preserved, and zero personal progress leaked
 */
import { expect, test, type APIRequestContext } from '@playwright/test';

import { showSidebar, sidebar } from '../e2e/shell.support.ts';

import {
  addWorkspace,
  person,
  register,
  type Schemas,
  signedIn,
  signInOnPage,
  watchErrors,
} from './support.ts';

test.use({ locale: 'en-US' });

const ID = '[0-9a-f-]{36}';
const ON_A_COURSE = new RegExp(`/w/${ID}/courses/(${ID})(/(${ID}))?$`);

function coursePlaceOf(address: string): { workspaceId: string; courseId: string } {
  const match = new RegExp(`/w/(${ID})/courses/(${ID})`).exec(
    new URL(address, 'http://here').pathname,
  );
  return { workspaceId: match?.[1] ?? '', courseId: match?.[2] ?? '' };
}

async function getJson<T>(api: APIRequestContext, url: string): Promise<T> {
  const res = await api.get(url);
  expect(res.ok(), `${url}: ${res.status()} ${await res.text()}`).toBe(true);
  return (await res.json()) as T;
}

test('built-in course template -> duplicate -> export/import preserves amounts & schedule without personal progress', async ({
  page,
  baseURL,
}, testInfo) => {
  const base = baseURL ?? '';
  const owner = person('course-l6');
  await register(base, owner);
  const api = await signedIn(base, owner);
  const workspaces = await getJson<Schemas['UserWorkspace'][]>(api, '/api/v1/users/me/workspaces');
  const home = workspaces[0]?.id ?? '';
  const second = addWorkspace(base, owner, 'Course L6 Import');

  const errors = watchErrors(page);
  await page.goto(`/w/${home}`);
  await signInOnPage(page, owner);

  // 1) Create a course from the built-in course template in the window of new things.
  await showSidebar(page);
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await expect(picker).toBeVisible();
  await picker.getByTestId('new-title').fill('Live L6 Study');
  const tplButton = picker.getByTestId('use-template-course-study-plan');
  await expect(tplButton).toBeVisible();
  await tplButton.click();

  await expect(page).toHaveURL(ON_A_COURSE);
  await expect(page.getByTestId('course-screen')).toBeVisible();
  const { courseId: srcId } = coursePlaceOf(page.url());
  expect(srcId).not.toBe('');

  // Verify structure, amounts, and shared schedule on the server.
  const srcUnits = await getJson<Schemas['UnitNode'][]>(
    api,
    `/api/v1/w/${home}/courses/${srcId}/units`,
  );
  const srcContentUnits = srcUnits.filter((n) => n.kind === 'unit');
  expect(srcUnits.filter((n) => n.kind === 'chapter')).toHaveLength(2);
  expect(srcContentUnits).toHaveLength(5);
  expect(srcContentUnits[0]?.amount).toBe(30);
  expect(srcContentUnits[0]?.amountUnit).toBe('minutes');
  expect(srcContentUnits[0]?.scheduledDate).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  const srcSched = await getJson<Schemas['CourseScheduleView']>(
    api,
    `/api/v1/w/${home}/courses/${srcId}/schedule`,
  );
  expect(srcSched.schedule?.studyDays).toBe(31);

  // 2) Record personal unit progress (completed, lastPosition, personal plannedDate).
  const firstUnitId = srcContentUnits[0]?.id ?? '';
  const p1 = await api.put(`/api/v1/w/${home}/courses/${srcId}/units/${firstUnitId}/progress`, {
    data: { completed: true, lastPosition: 'live-secret-pos' },
  });
  expect(p1.ok()).toBe(true);
  const p2 = await api.put(`/api/v1/w/${home}/courses/${srcId}/units/${firstUnitId}/schedule`, {
    data: { plannedDate: '2026-12-28' },
  });
  expect(p2.ok()).toBe(true);

  // 3) Duplicate the course via sidebar menu.
  await showSidebar(page);
  await sidebar(page)
    .getByRole('button', { name: 'Menu of the course Live L6 Study', exact: true })
    .click();
  await page.getByRole('menuitem', { name: 'Duplicate course' }).click();
  await expect(page).toHaveURL(ON_A_COURSE);
  await expect.poll(() => coursePlaceOf(page.url()).courseId).not.toBe(srcId);
  const dupId = coursePlaceOf(page.url()).courseId;
  await expect(sidebar(page).getByRole('link', { name: 'Live L6 Study copy' })).toBeVisible();

  const dupUnits = await getJson<Schemas['UnitNode'][]>(
    api,
    `/api/v1/w/${home}/courses/${dupId}/units`,
  );
  expect(dupUnits.map((n) => [n.title, n.amount, n.amountUnit, n.scheduledDate])).toEqual(
    srcUnits.map((n) => [n.title, n.amount, n.amountUnit, n.scheduledDate]),
  );
  const dupSched = await getJson<Schemas['CourseScheduleView']>(
    api,
    `/api/v1/w/${home}/courses/${dupId}/schedule`,
  );
  const expectedSchedule = {
    startDate: srcSched.schedule?.startDate,
    targetDate: srcSched.schedule?.targetDate,
    studyDays: srcSched.schedule?.studyDays,
  };
  expect(dupSched.schedule).toMatchObject(expectedSchedule);
  expect(dupSched.units.every((u) => u.myPlannedDate === null && !u.completed)).toBe(true);
  const dupProgress = await getJson<Schemas['UnitProgress'][]>(
    api,
    `/api/v1/w/${home}/courses/${dupId}/progress`,
  );
  expect(dupProgress).toEqual([]);

  // 4) Export the original course archive and import it into a second workspace.
  const saving = page.waitForEvent('download');
  await sidebar(page)
    .getByRole('button', { name: 'Menu of the course Live L6 Study', exact: true })
    .click();
  // Under the heading of what it saves, the archive's line, beside the Markdown's.
  await page.getByRole('menuitem', { name: 'As a .fvoci archive' }).click();
  const archive = testInfo.outputPath('live-course-l6.fvoci');
  await (await saving).saveAs(archive);

  await page.goto(`/w/${second}`);
  await sidebar(page).getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');
  await settings.getByTestId('import-archive').setInputFiles(archive);
  await expect(settings).toContainText('Courses: 1');
  await expect(settings).toContainText('Units: 5');

  const importedCourses = await getJson<Schemas['Container'][]>(api, `/api/v1/w/${second}/courses`);
  expect(importedCourses).toHaveLength(1);
  const impId = importedCourses[0]?.id ?? '';
  const impUnits = await getJson<Schemas['UnitNode'][]>(
    api,
    `/api/v1/w/${second}/courses/${impId}/units`,
  );
  expect(impUnits.map((n) => [n.title, n.amount, n.amountUnit, n.scheduledDate])).toEqual(
    srcUnits.map((n) => [n.title, n.amount, n.amountUnit, n.scheduledDate]),
  );
  const impSched = await getJson<Schemas['CourseScheduleView']>(
    api,
    `/api/v1/w/${second}/courses/${impId}/schedule`,
  );
  expect(impSched.schedule).toMatchObject(expectedSchedule);
  expect(impSched.units.every((u) => u.myPlannedDate === null && !u.completed)).toBe(true);
  const impProgress = await getJson<Schemas['UnitProgress'][]>(
    api,
    `/api/v1/w/${second}/courses/${impId}/progress`,
  );
  expect(impProgress).toEqual([]);

  await api.dispose();
  expect(errors).toEqual([]);
});
