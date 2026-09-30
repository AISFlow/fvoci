import { expect, type Page } from "@playwright/test";

/** Prove a real URL booted the Vue candidate, rather than its old React viewer. */
export async function expectVueViewer(page: Page): Promise<void> {
  await expect(page.locator("[data-attachment-viewer]")).toBeVisible();
  expect(await page.locator("#root").evaluate((root) => Boolean((root as HTMLElement & { __vue_app__?: unknown }).__vue_app__))).toBe(true);
}
