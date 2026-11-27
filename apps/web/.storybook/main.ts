import type { StorybookConfig } from '@storybook/svelte-vite';

const config: StorybookConfig = {
  framework: '@storybook/svelte-vite',
  stories: ['../stories/**/*.stories.ts'],
  addons: [],
  core: {
    // Nothing leaves the machine: no usage statistics, crash reports or news.
    disableTelemetry: true,
    enableCrashReports: false,
    disableWhatsNewNotifications: true,
    disableProjectJson: true,
  },
  viteFinal(viteConfig) {
    // The app's build steps (hashed names for the server, notices, compression)
    // shape the product's output directory and do not apply to Storybook's.
    return {
      ...viteConfig,
      plugins: (viteConfig.plugins ?? [])
        .flat()
        .filter(
          (plugin) =>
            !(
              plugin &&
              typeof plugin === 'object' &&
              'name' in plugin &&
              plugin.name.startsWith('fvoci:')
            ),
        ),
      experimental: {},
      build: {
        ...viteConfig.build,
        sourcemap: false,
        license: false,
        rolldownOptions: {},
        assetsDir: 'assets',
      },
    };
  },
};

export default config;
