/**
 * The layering rule of src/lib, run through ESLint itself on small sources:
 * a feature importing another feature fails, and so does shared code
 * importing a feature.
 */
import path from 'node:path';

import { Linter } from 'eslint';
import { describe, expect, it } from 'vitest';

import layers from '../../tools/lint/layers.ts';

const ROOT = path.resolve(import.meta.dirname, '../..');

function messages(file: string, code: string, rule = 'fvoci/feature-boundaries'): string[] {
  const linter = new Linter({ cwd: ROOT });
  return linter
    .verify(
      code,
      [
        {
          files: ['**/*.ts'],
          plugins: { fvoci: layers },
          rules: { [rule]: 'error' },
        },
      ],
      { filename: path.join(ROOT, file) },
    )
    .map((message) => message.message);
}

describe('feature boundaries', () => {
  const inBoard = 'src/lib/features/board/Board.ts';

  it('lets a feature import from itself and from the shared layers', () => {
    const code = [
      "import { a } from './parts/a.ts';",
      "import { b } from '$lib/features/board/b.ts';",
      "import { c } from '$lib/logic/filter.ts';",
      "import { d } from '../../ui/button/index.js';",
      "import { e } from 'svelte';",
    ].join('\n');
    expect(messages(inBoard, code)).toEqual([]);
  });

  it('refuses an import from another feature, by alias or by relative path', () => {
    expect(messages(inBoard, "import { x } from '$lib/features/card/Card.ts';")).toEqual([
      expect.stringContaining('The feature "board" imports from the feature "card"'),
    ]);
    expect(messages(inBoard, "import { x } from '../card/Card.ts';")).toHaveLength(1);
    expect(messages(inBoard, "export * from '../card/Card.ts';")).toHaveLength(1);
    expect(messages(inBoard, "const x = await import('$lib/features/card/Card.ts');")).toHaveLength(
      1,
    );
  });

  it('counts the first folder under features as the feature', () => {
    const inKanban = 'src/lib/features/views/kanban/Kanban.ts';
    expect(messages(inKanban, "import { x } from '../table/Table.ts';")).toEqual([]);
    expect(messages(inKanban, "import { x } from '../../board/Board.ts';")).toHaveLength(1);
  });

  it('refuses shared code that imports a feature', () => {
    expect(
      messages('src/lib/logic/sort.ts', "import { x } from '$lib/features/board/b.ts';"),
    ).toEqual([expect.stringContaining('lib/logic imports from the feature "board"')]);
    expect(
      messages('src/lib/state/boards.ts', "import { x } from '../features/board/b.ts';"),
    ).toHaveLength(1);
  });

  it('leaves the app entry free to import any feature', () => {
    expect(messages('src/app/App.ts', "import { x } from '$lib/features/board/b.ts';")).toEqual([]);
  });
});

function deviceTime(code: string): string[] {
  const linter = new Linter({ cwd: ROOT });
  return linter
    .verify(
      code,
      [
        {
          files: ['**/*.ts'],
          plugins: { fvoci: layers },
          rules: { 'fvoci/no-device-time': 'error' },
        },
      ],
      { filename: path.join(ROOT, 'src/lib/logic/example.ts') },
    )
    .map((message) => message.message);
}

describe('no device time in the logic', () => {
  it('refuses the methods of Date that answer in the time zone of the device', () => {
    for (const method of [
      'getHours',
      'getDate',
      'getDay',
      'getMonth',
      'getFullYear',
      'getTimezoneOffset',
    ]) {
      expect(deviceTime(`const x = date.${method}();`)).toEqual([
        expect.stringContaining(`\`${method}\` works in the time zone of the device`),
      ]);
    }
    expect(deviceTime('date.setHours(0, 0, 0, 0);')).toHaveLength(1);
    expect(deviceTime('date.setDate(date.getUTCDate() + 1);')).toHaveLength(1);
    expect(deviceTime("const text = date.toLocaleDateString('en');")).toHaveLength(1);
    expect(deviceTime("const text = date.toLocaleTimeString('en');")).toHaveLength(1);
  });

  it('refuses a Date built from parts or from text', () => {
    expect(deviceTime('const d = new Date(2026, 2, 5);')).toEqual([
      expect.stringContaining('reads its arguments as device time'),
    ]);
    expect(deviceTime('const d = new Date(2026, 2, 5, 12, 0);')).toHaveLength(1);
    expect(deviceTime("const d = new Date('December 7, 2023');")).toHaveLength(1);
    expect(deviceTime('const d = new Date(`${year}-03-05T00:00`);')).toHaveLength(1);
  });

  it('refuses formatting without a time zone, and asking the device for its zone', () => {
    expect(deviceTime("new Intl.DateTimeFormat('en').format(moment);")).toEqual([
      expect.stringContaining('formats in the time zone of the device'),
    ]);
    expect(
      deviceTime("new Intl.DateTimeFormat('en', { month: 'long' }).format(moment);"),
    ).toHaveLength(1);
    expect(deviceTime('Intl.DateTimeFormat().format(moment);')).toHaveLength(1);
    expect(deviceTime('const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;')).toEqual(
      expect.arrayContaining([expect.stringContaining('read in lib/regional.ts only')]),
    );
  });

  it('lets through what does not depend on the device', () => {
    const code = [
      'const a = new Date(instant).getUTCHours();',
      'const b = new Date(instant).getTime();',
      'const c = Date.UTC(2026, 2, 5);',
      'const d = Date.now();',
      'const e = new Date();',
      "const f = new Intl.DateTimeFormat('en', { timeZone, month: 'long' }).format(instant);",
      "const g = new Intl.DateTimeFormat('en', { ...options, timeZone: 'UTC' }).format(instant);",
      "const h = new Intl.DateTimeFormat('en', { hour: 'numeric', timeZone: 'UTC' }).resolvedOptions().hourCycle;",
      'const i = new Date(instant).toISOString();',
    ].join('\n');
    expect(deviceTime(code)).toEqual([]);
  });
});

describe('the time zone of the device', () => {
  it('is read in src/lib/regional.ts and nowhere else in the app', async () => {
    const { readdirSync, readFileSync } = await import('node:fs');
    const asksTheDevice = /resolvedOptions\(\)\s*\.timeZone|getLocalTimeZone|getTimezoneOffset/;
    const found: string[] = [];
    const walk = (folder: string): void => {
      for (const entry of readdirSync(path.join(ROOT, folder), { withFileTypes: true })) {
        const file = `${folder}/${entry.name}`;
        if (entry.isDirectory()) {
          if (file !== 'src/lib/i18n/paraglide') walk(file);
        } else if (/\.(ts|svelte)$/.test(entry.name)) {
          if (asksTheDevice.test(readFileSync(path.join(ROOT, file), 'utf8'))) found.push(file);
        }
      }
    };
    walk('src');
    expect(found).toEqual(['src/lib/regional.ts']);
  });
});

describe('the order of the layers', () => {
  const order = (file: string, code: string): string[] => messages(file, code, 'fvoci/layer-order');

  it('lets a layer import from itself and from the layers below it', () => {
    expect(
      order(
        'src/lib/components/properties/PropertyValue.ts',
        [
          "import { a } from '$lib/logic/wire.ts';",
          "import { b } from '$lib/ui/button/index.js';",
          "import { c } from '$lib/components/pick-list/PickList.ts';",
          "import { d } from '$lib/layout/stage.svelte.ts';",
          "import { e } from '$lib/regional.ts';",
        ].join('\n'),
      ),
    ).toEqual([]);
    expect(order('src/lib/state/board.ts', "import { a } from '$lib/editor/schema.ts';")).toEqual(
      [],
    );
  });

  it('refuses an import from a layer that is not below', () => {
    expect(order('src/lib/logic/sort.ts', "import { a } from '$lib/state/board.ts';")).toEqual([
      expect.stringContaining('lib/logic imports from lib/state'),
    ]);
    expect(
      order('src/lib/state/board.ts', "import { a } from '$lib/ui/button/index.js';"),
    ).toHaveLength(1);
    expect(
      order('src/lib/editor/render.ts', "import { a } from '../components/x.ts';"),
    ).toHaveLength(1);
  });

  it('keeps the shared components from the state although it is below them', () => {
    expect(
      order(
        'src/lib/components/properties/PropertyValue.ts',
        "import { a } from '$lib/state/x.ts';",
      ),
    ).toEqual([expect.stringContaining('takes values and callbacks')]);
  });

  it('wants a new folder under src/lib to be entered in the table', () => {
    expect(order('src/lib/newthing/a.ts', 'export const a = 1;')).toEqual([
      expect.stringContaining('not a known layer'),
    ]);
  });

  it('leaves features and the app to the other rule', () => {
    expect(order('src/lib/features/board/a.ts', "import { a } from '$lib/state/x.ts';")).toEqual(
      [],
    );
    expect(order('src/app/App.ts', "import { a } from '$lib/features/board/b.ts';")).toEqual([]);
  });
});

describe('screens do not reach a server themselves', () => {
  const server = (file: string, code: string): string[] =>
    messages(file, code, 'fvoci/no-direct-server');
  const inFeature = 'src/lib/features/board/Board.ts';

  it('refuses the globals that talk to a server', () => {
    expect(server(inFeature, "await fetch('/api/v1/boards');")).toEqual([
      expect.stringContaining('`fetch` reaches a server'),
    ]);
    expect(server(inFeature, "await window.fetch('/api/v1/boards');")).toHaveLength(1);
    expect(server(inFeature, 'new XMLHttpRequest();')).toHaveLength(1);
    expect(server(inFeature, "new WebSocket('/ws');")).toHaveLength(1);
    expect(server(inFeature, "new EventSource('/events');")).toHaveLength(1);
    expect(server(inFeature, "navigator.sendBeacon('/x');")).toHaveLength(1);
  });

  it('refuses an import of an implementation of the boundary', () => {
    expect(server(inFeature, "import { a } from '$lib/state/memory/backend.ts';")).toEqual([
      expect.stringContaining('implementation of the data boundary'),
    ]);
    expect(server(inFeature, "import { a } from '$lib/api/client.ts';")).toHaveLength(1);
    expect(server(inFeature, "const a = await import('../../state/memory/seed.ts');")).toHaveLength(
      1,
    );
  });

  it('lets a screen use the interface', () => {
    expect(
      server(
        inFeature,
        [
          "import { a } from '$lib/state/index.ts';",
          "import { b } from '$lib/state/board.svelte.ts';",
        ].join('\n'),
      ),
    ).toEqual([]);
  });
});
