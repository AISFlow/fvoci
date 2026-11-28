/**
 * Course L6 end to end on the in-memory backend:
 * - Creating a course from the built-in course template from the window of new things
 * - Recording personal unit completion
 * - Duplicating the course via sidebar menu (units kept, personal completion excluded)
 * - Exporting the course archive and importing it back (units kept, personal completion excluded)
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import { hideSidebar, openApp, showSidebar, sidebar } from './shell.support.ts';

const ID = '[0-9a-f-]{36}';
const ON_A_COURSE = new RegExp(`/w/${ID}/courses/(${ID})(/(${ID}))?$`);

function courseIdOf(address: string): string {
  const match = new RegExp(`/courses/(${ID})`).exec(new URL(address, 'http://here').pathname);
  return match?.[1] ?? '';
}

const unitTree = (page: Page): Locator => page.getByTestId('unit-tree');

async function showTree(page: Page): Promise<void> {
  const opener = page.getByTestId('open-unit-tree');
  await expect
    .poll(async () => {
      if ((await opener.isVisible()) && (await opener.getAttribute('aria-expanded')) !== 'true') {
        await opener.click();
      }
      return unitTree(page).isVisible();
    })
    .toBe(true);
}

async function hideTree(page: Page): Promise<void> {
  const opener = page.getByTestId('open-unit-tree');
  if (!(await opener.isVisible()) || (await opener.getAttribute('aria-expanded')) !== 'true') {
    return;
  }
  await page.getByTestId('unit-tree-panel').press('Escape');
  await expect(opener).toHaveAttribute('aria-expanded', 'false');
  await expect(page.getByTestId('unit-tree-panel')).toBeHidden();
}

test('creates a course from built-in template, duplicates, and exports/imports without personal progress', async ({
  page,
}, testInfo) => {
  await openApp(page);
  await showSidebar(page);
  await sidebar(page).getByTestId('add-board').click();
  const picker = page.getByTestId('template-picker');
  await expect(picker).toBeVisible();
  await picker.getByTestId('new-title').fill('L6 Study Plan');
  const tplButton = picker.getByTestId('use-template-course-study-plan');
  await expect(tplButton).toBeVisible();
  await tplButton.click();

  await expect(page).toHaveURL(ON_A_COURSE);
  await expect(page.getByTestId('course-screen')).toBeVisible();
  const originalId = courseIdOf(page.url());
  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('0% (0/5 units)');
  await expect(
    unitTree(page).getByRole('treeitem', { name: 'Define Goals and Scope' }),
  ).toBeVisible();
  await hideTree(page);

  // Mark the first unit completed on the original course.
  const toggle = page.getByTestId('unit-complete-toggle');
  await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('20% (1/5 units)');
  await hideTree(page);

  // Duplicate the course from the sidebar menu: units preserved, personal completion excluded.
  await showSidebar(page);
  await sidebar(page)
    .getByRole('button', { name: 'Menu of the course L6 Study Plan', exact: true })
    .click();
  await page.getByRole('menuitem', { name: 'Duplicate course' }).click();
  await expect(page).toHaveURL(ON_A_COURSE);
  await expect.poll(() => courseIdOf(page.url())).not.toBe(originalId);
  const dupId = courseIdOf(page.url());
  await showSidebar(page);
  await expect(sidebar(page).getByRole('link', { name: 'L6 Study Plan copy' })).toBeVisible();
  await hideSidebar(page);
  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('0% (0/5 units)');
  await hideTree(page);

  // Export the original course and import it back: units preserved, personal completion excluded.
  await showSidebar(page);
  const saving = page.waitForEvent('download');
  await sidebar(page)
    .getByRole('button', { name: 'Menu of the course L6 Study Plan', exact: true })
    .click();
  await page.getByRole('menuitem', { name: 'As a .fvoci archive' }).click();
  const archive = testInfo.outputPath('course-l6.fvoci');
  await (await saving).saveAs(archive);

  await showSidebar(page);
  await sidebar(page).getByTestId('open-settings').click();
  const settings = page.getByTestId('settings-dialog');
  await settings.getByTestId('import-archive').setInputFiles(archive);
  await expect(settings).toContainText('Courses: 1');
  await expect(settings).toContainText('Units: 5');
  await settings.getByRole('link', { name: 'L6 Study Plan' }).first().click();
  await expect(settings).toBeHidden();
  await expect(page).toHaveURL(ON_A_COURSE);
  await expect.poll(() => courseIdOf(page.url())).not.toBe(dupId);
  await expect(page.getByTestId('course-screen')).toBeVisible();
  await showTree(page);
  await expect(page.getByTestId('course-progress-text')).toHaveText('0% (0/5 units)');
});
