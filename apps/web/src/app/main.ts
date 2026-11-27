import { mount } from 'svelte';

import '../styles/app.css';
import { noteChunkFailure, reloadForNewVersion } from '$lib/chunk-reload.svelte.ts';
import { watchDisplay } from '$lib/display.svelte.ts';
import { startCodeLines } from '$lib/editor/code-lines.svelte.ts';
import { loadFontFaces } from '$lib/fonts.js';
import { m } from '$lib/i18n/paraglide/messages.js';
import { watchKeyboard } from '$lib/layout/keyboard.ts';
import { insets, page } from '$lib/layout/room.svelte.ts';
import { watchSystemTheme } from '$lib/theme.svelte.js';

import { startLocale } from './locale.ts';
import { isSharedRoute, readBasePath, Router } from './router.svelte.ts';

const target = document.getElementById('app');
if (!target) throw new Error('index.html has no #app element');

const locale = startLocale();
watchSystemTheme();
watchDisplay();
startCodeLines();
watchKeyboard((height) => {
  insets.keyboard = height;
});
insets.watch();
// The page is what is inside the edges the device keeps: #app has them as padding.
page.attach(target);
loadFontFaces(locale);

const router = new Router(readBasePath(document));
router.listen();

function renderEntryFailure(appTarget: HTMLElement): void {
  const main = document.createElement('main');
  main.className = 'grid min-h-dvh place-items-center p-4';
  const box = document.createElement('div');
  box.className = 'text-center';
  const alert = document.createElement('p');
  alert.className = 'text-lg font-medium';
  alert.setAttribute('role', 'alert');
  alert.textContent = m.app_load_failed();
  const retry = document.createElement('button');
  retry.type = 'button';
  retry.className =
    'mt-4 inline-flex h-9 items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground roomy:min-h-touch';
  retry.textContent = m.app_retry();
  retry.addEventListener('click', () => {
    location.reload();
  });
  box.append(alert, retry);
  main.append(box);
  appTarget.replaceChildren(main);
}

async function mountEntry(appTarget: HTMLElement): Promise<Record<string, unknown>> {
  try {
    if (router.route.kind === 'passwordReset' || router.route.kind === 'passwordForgot') {
      const { default: ResetPasswordApp } = await import('./ResetPasswordApp.svelte');
      return mount(ResetPasswordApp, { target: appTarget, props: { router } });
    }
    if (router.route.kind === 'emailVerify') {
      const { default: VerifyEmailApp } = await import('./VerifyEmailApp.svelte');
      return mount(VerifyEmailApp, { target: appTarget, props: { router } });
    }
    if (router.route.kind === 'emailConfirm') {
      const { default: ConfirmEmailApp } = await import('./ConfirmEmailApp.svelte');
      return mount(ConfirmEmailApp, { target: appTarget, props: { router } });
    }
    if (router.route.kind === 'emailRevert') {
      const { default: RevertEmailApp } = await import('./RevertEmailApp.svelte');
      return mount(RevertEmailApp, { target: appTarget, props: { router } });
    }
    if (isSharedRoute(router.route)) {
      const { default: SharedApp } = await import('./SharedApp.svelte');
      return mount(SharedApp, { target: appTarget, props: { router } });
    }
    const { default: App } = await import('./App.svelte');
    return mount(App, { target: appTarget, props: { router } });
  } catch {
    if (await noteChunkFailure()) {
      await reloadForNewVersion();
      return {};
    }
    renderEntryFailure(appTarget);
    return {};
  }
}

export default mountEntry(target);
