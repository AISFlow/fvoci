import path from 'node:path';

import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vitest/config';

import { cspHashes } from './tools/build/inline-scripts.ts';
import { checkHashedNames, precompress } from './tools/build/output.ts';
import { pdfjsData } from './tools/build/pdfjs-data.ts';
import { rhwpWasm } from './tools/build/rhwp-wasm.ts';
import { studioVersion } from './tools/build/rhwp-studio.ts';
import { pptxRenderer } from './tools/build/pptx-renderer.ts';
import { thirdPartyNotices } from './tools/licenses/notices.ts';
import { workerPackages } from './tools/licenses/worker-packages.ts';
import { vitestWorkers } from './tools/test-cpus.ts';

/**
 * The server serves `index.html` for every page of the app and everything
 * else from `static/`. It also decides the path the app lives under, at run
 * time: it replaces this placeholder in `index.html` with that path (empty for
 * the root). So the build must not fix a base path: `index.html` refers to its
 * files through the placeholder, and scripts and style sheets refer to each
 * other relatively.
 */
const BASE_URL_PLACEHOLDER = '{{.BaseURL}}';

/**
 * The document the file viewer draws in (src/viewer-frame/), a page of its
 * own. It is built under `static/`, the one folder the server serves as
 * files, and is served as it is: it refers to its files relatively, as it
 * lies beside them.
 */
const VIEWER_FRAME = 'static/viewer-frame.html';

/**
 * The document the HWP editor runs in (src/editor-frame/), a page of its own
 * as the viewer's is. The editor is rhwp-studio, brought into the repository
 * as the package `@fvoci/rhwp-studio` and built from its source here: the
 * names below are what the frame's program loads of it, and what the editor's
 * source asks of a build of its own.
 */
const EDITOR_FRAME = 'static/editor-frame.html';
const FRAMES = [VIEWER_FRAME, EDITOR_FRAME];
const STUDIO = path.resolve(
  import.meta.dirname,
  'node_modules/@fvoci/rhwp-studio/upstream/rhwp-studio',
);
const EDITOR_SOURCE = path.resolve(import.meta.dirname, 'src/editor-frame');
const STUDIO_ALIASES = [
  { find: 'rhwp-studio:page', replacement: `${STUDIO}/index.html?raw` },
  { find: 'rhwp-studio:style', replacement: `${STUDIO}/src/style.css` },
  { find: 'rhwp-studio:theme-init', replacement: `${STUDIO}/public/theme-init.js` },
  { find: 'rhwp-studio:locale-init', replacement: `${STUDIO}/public/locale-init.js` },
  { find: 'rhwp-studio:main', replacement: `${STUDIO}/src/main.ts` },
  // The engine: `@rhwp/core` from npm, given its bytes and without the PDF import (rhwp.ts).
  { find: '@wasm/rhwp.js', replacement: `${EDITOR_SOURCE}/rhwp.ts` },
  // The editor's other renderer, CanvasKit, is left out (no-canvaskit.ts), and so are the
  // two plugins its build can do without: neither is in the repository.
  { find: '@/view/canvaskit-renderer', replacement: `${EDITOR_SOURCE}/no-canvaskit.ts` },
  { find: /^@\//, replacement: `${STUDIO}/src/` },
];
/** What the editor's source reads of its build: its version, and what is left out of it. */
const STUDIO_CONSTANTS = {
  __APP_VERSION__: JSON.stringify(studioVersion()),
  __RHWP_DISABLE_EXTERNAL_WEBFONTS__: 'true',
  __RHWP_HWPCTRL__: 'false',
  __RHWP_AI__: 'false',
};

/** Names of the output files: hexadecimal hashes, which the server recognises (see `build`). */
const OUTPUT_NAMES = {
  hashCharacters: 'hex',
  entryFileNames: 'static/[name]-[hash:16].js',
  chunkFileNames: 'static/[name]-[hash:16].js',
  assetFileNames: 'static/[name]-[hash:16][extname]',
} as const;

export default defineConfig(({ command }) => ({
  define: STUDIO_CONSTANTS,
  plugins: [
    tailwindcss(),
    svelte(),
    svelteTesting(),
    pdfjsData(),
    rhwpWasm(),
    pptxRenderer(),
    thirdPartyNotices(),
    checkHashedNames(),
    precompress(),
    cspHashes(),
  ],
  resolve: {
    alias: [
      { find: '$lib', replacement: path.resolve(import.meta.dirname, 'src/lib') },
      // The diagram library's ELK layout engine is under a licence the project
      // does not allow, so it is left out of the build: this stub takes its
      // place (see the file). The notices step fails the build if the real
      // package is ever bundled.
      {
        find: /^elkjs(\/.*)?$/,
        replacement: path.resolve(import.meta.dirname, 'src/lib/editor/no-elk.ts'),
      },
      // The file viewer's PowerPoint renderer draws charts with echarts; the
      // viewer shows a box in their place, and this stub takes the library's
      // (see the file and tools/build/pptx-renderer.ts).
      {
        find: /^echarts(\/.*)?$/,
        replacement: path.resolve(import.meta.dirname, 'src/viewer-frame/no-echarts.ts'),
      },
      ...STUDIO_ALIASES,
    ],
  },
  experimental:
    command === 'build'
      ? {
          renderBuiltUrl(fileName, { hostId, hostType }) {
            if (hostType !== 'html') return { relative: true };
            if (FRAMES.includes(hostId)) {
              return `./${path.posix.relative(path.posix.dirname(hostId), fileName)}`;
            }
            return `${BASE_URL_PLACEHOLDER}/${fileName}`;
          },
        }
      : {},
  build: {
    outDir: 'dist',
    assetsDir: 'static',
    // Every asset is a file of its own. Vite would otherwise write a small
    // one (the smallest formula font) into the style sheet as a `data:`
    // address, which the page's content security policy does not allow.
    assetsInlineLimit: 0,
    // Raw data for tools/licenses: which packages ended up in the bundle.
    license: { fileName: '.vite/license.json' },
    rolldownOptions: {
      input: {
        index: path.resolve(import.meta.dirname, 'index.html'),
        'viewer-frame': path.resolve(import.meta.dirname, VIEWER_FRAME),
        'editor-frame': path.resolve(import.meta.dirname, EDITOR_FRAME),
      },
      // Hexadecimal, because that is what the server recognises as a
      // content hash. Sixteen digits: a hash of letters a-f only, which the
      // server would not recognise, has a chance of 1 in 6.6 million.
      output: {
        ...OUTPUT_NAMES,
        codeSplitting: {
          groups: [
            {
              name: 'i18n-shared',
              test: /src\/lib\/i18n\/paraglide\//,
              minShareCount: 2,
            },
            {
              name: 'app-shared',
              test: /src\/lib\/(state|logic|components|ui)\//,
              minShareCount: 2,
              maxModuleSize: 4096,
            },
          ],
        },
      },
    },
  },
  // pdf.js's and rhwp's workers, which the viewer's frame starts from their
  // own text (src/viewer-frame/pdf-worker.ts, hwp-worker.ts): classic
  // scripts, as a `blob:` worker of an opaque origin cannot load modules.
  worker: {
    format: 'iife',
    // What each worker bundles, for the notices (their step does not see inside a worker).
    plugins: () => [rhwpWasm(), workerPackages()],
    rolldownOptions: { output: OUTPUT_NAMES },
  },
  test: {
    // FVOCI_TEST_CPUS, when it is set (tools/test-cpus.ts); read under Vitest only, so
    // that a wrong value stops no build.
    maxWorkers: process.env.VITEST ? vitestWorkers() : undefined,
    projects: [
      {
        extends: true,
        test: {
          name: 'logic',
          environment: 'node',
          include: [
            'tests/logic/**/*.test.ts',
            'tests/editor/**/*.test.ts',
            'tests/tokens/**/*.test.ts',
            'tests/tools/**/*.test.ts',
          ],
        },
      },
      {
        extends: true,
        test: {
          name: 'ui',
          environment: 'jsdom',
          include: ['tests/ui/**/*.test.ts', 'tests/state/**/*.test.ts'],
          setupFiles: ['tests/ui/setup.ts'],
        },
      },
    ],
  },
}));
