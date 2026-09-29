// Options for Nuxt UI's Vite plugin (vite.config.ts), shared with the test that
// checks the index.html colors style against the plugin's own output.
//
// Component library only: no Nuxt server. Components and composables are
// imported explicitly in the Vue sources (no generated auto-import .d.ts), the
// React app has no dark mode, fonts stay off (the plugin's Vue mode default,
// no Google Fonts fetch), and icons are bundled from the installed
// @iconify-json/lucide set, never fetched from the Iconify API at run time.
export const nuxtUiUserOptions = {
  colorMode: false,
  autoImport: false as const,
  components: false as const,
  ui: {
    colors: {
      primary: "teal",
      neutral: "slate",
    },
  },
  // Theme CSS only for the components the sources use (and their dependencies).
  experimental: { componentDetection: true },
  icon: {
    clientBundle: {
      scan: { globInclude: ["src/vue/**/*.{vue,ts}"], globExclude: ["**/*.test.ts"] },
    },
  },
};
