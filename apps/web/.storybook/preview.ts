import type { Preview } from '@storybook/svelte';
import { STORY_FINISHED } from 'storybook/internal/core-events';
import { addons } from 'storybook/preview-api';

import { loadFontFaces } from '$lib/fonts.js';
import { setLocale } from '$lib/i18n/paraglide/runtime.js';
import { page } from '$lib/layout/room.svelte.ts';
import { applyTheme } from '$lib/theme.svelte.js';

import '../src/styles/app.css';
import './preview.css';
import './clock.ts';
import StoryLocale from './StoryLocale.svelte';

// The browser check waits for Storybook's result, including failures in play functions.
addons.getChannel().on(STORY_FINISHED, (result: { storyId: string; status: string }) => {
  document.documentElement.dataset.storyFinished = result.storyId;
  document.documentElement.dataset.storyResult = result.status;
});

// Parts that open as a sheet on a compact page ask the page for its width.
page.attach(document.documentElement);

const preview: Preview = {
  decorators: [
    (_story, context) => ({
      Component: StoryLocale,
      props: { locale: context.globals.locale === 'ko' ? 'ko' : 'en' },
    }),
  ],
  globalTypes: {
    locale: {
      description: 'Language',
      toolbar: {
        icon: 'globe',
        items: [
          { value: 'en', title: 'English' },
          { value: 'ko', title: '한국어' },
        ],
        dynamicTitle: true,
      },
    },
    theme: {
      description: 'Theme',
      toolbar: {
        icon: 'contrast',
        items: [
          { value: 'light', title: 'Light' },
          { value: 'dark', title: 'Dark' },
        ],
        dynamicTitle: true,
      },
    },
    textSize: {
      description: 'Root text size',
      toolbar: {
        icon: 'zoom',
        items: ['100', '150', '200'].map((value) => ({ value, title: `${value}%` })),
        dynamicTitle: true,
      },
    },
  },
  initialGlobals: { locale: 'en', theme: 'light', textSize: '100' },
  parameters: { layout: 'padded' },
  async beforeEach({ globals }) {
    delete document.documentElement.dataset.storyFinished;
    delete document.documentElement.dataset.storyResult;
    const locale = globals.locale === 'ko' ? 'ko' : 'en';
    await setLocale(locale, { reload: false });
    document.documentElement.lang = locale;
    // The app's own fonts, as in the product: the family of the language first.
    loadFontFaces(locale);
    document.documentElement.style.fontSize = `${Number(globals.textSize) || 100}%`;
    applyTheme(globals.theme === 'dark' ? 'dark' : 'light');
  },
};

export default preview;
