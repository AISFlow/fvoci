/**
 * ESLint rules of this project: the layering of src/lib, keeping the screens
 * away from the server (`no-direct-server`), and keeping the time zone of the
 * device out of the logic (`no-device-time`).
 *
 * - A feature (`src/lib/features/<name>/`) does not import from another
 *   feature. What two features share moves down into a shared layer.
 * - The shared layers do not import from a feature.
 * - The shared layers are stacked (`layer-order`): a layer imports only from
 *   the layers below it. See `LAYERS`.
 *
 * The rules look at the path an import resolves to, so `$lib/features/x` and
 * `../x` are treated alike.
 */
import path from 'node:path';

import type { ESLint, Rule } from 'eslint';

const LIB = `${path.sep}src${path.sep}lib${path.sep}`;

interface Place {
  layer: string;
  /** The first folder under `features`; undefined in any other layer. */
  feature: string | undefined;
}

/** Where under src/lib a file lies, if it does. */
function place(file: string): Place | undefined {
  const at = file.lastIndexOf(LIB);
  if (at === -1) return undefined;
  const [first = '', second] = file.slice(at + LIB.length).split(path.sep);
  // A file directly under src/lib is a layer of its own, named without the
  // extension (imports write `utils.js` for `utils.ts`).
  const layer = first.replace(/\.(ts|js)$/, '');
  return { layer, feature: layer === 'features' ? second : undefined };
}

/** The file path an import specifier points at, for `$lib/...` and relative specifiers. */
function resolve(specifier: string, importer: string): string | undefined {
  if (specifier.startsWith('$lib/')) {
    const at = importer.lastIndexOf(LIB);
    if (at === -1) return undefined;
    return path.join(importer.slice(0, at + LIB.length), specifier.slice('$lib/'.length));
  }
  if (specifier.startsWith('.')) return path.resolve(path.dirname(importer), specifier);
  return undefined;
}

const featureBoundaries: Rule.RuleModule = {
  meta: {
    type: 'problem',
    docs: {
      description: 'features do not import each other; shared layers do not import features',
    },
    schema: [],
    messages: {
      crossFeature:
        'The feature "{{from}}" imports from the feature "{{to}}". Move what they share into lib/logic, lib/state, lib/api or lib/ui.',
      sharedImportsFeature:
        'lib/{{layer}} imports from the feature "{{feature}}". Shared code must not depend on a feature.',
    },
  },
  create(context) {
    const from = place(context.filename);
    if (!from) return {};

    function check(node: Rule.Node, source: unknown): void {
      const specifier =
        typeof source === 'object' && source !== null && 'value' in source
          ? source.value
          : undefined;
      if (typeof specifier !== 'string') return;
      const target = resolve(specifier, context.filename);
      const to = target === undefined ? undefined : place(target);
      if (to?.feature === undefined) return;

      if (from?.feature === undefined) {
        context.report({
          node,
          messageId: 'sharedImportsFeature',
          data: { layer: from?.layer ?? '', feature: to.feature },
        });
      } else if (to.feature !== from.feature) {
        context.report({
          node,
          messageId: 'crossFeature',
          data: { from: from.feature, to: to.feature },
        });
      }
    }

    return {
      ImportDeclaration: (node) => {
        check(node, node.source);
      },
      ExportNamedDeclaration: (node) => {
        check(node, node.source);
      },
      ExportAllDeclaration: (node) => {
        check(node, node.source);
      },
      ImportExpression: (node) => {
        check(node, node.source);
      },
    };
  },
};

/** Methods of `Date` that answer in, or are set in, the time zone of the device. */
const DEVICE_TIME_METHODS = new Set([
  'getDate',
  'getDay',
  'getFullYear',
  'getHours',
  'getMilliseconds',
  'getMinutes',
  'getMonth',
  'getSeconds',
  'getTimezoneOffset',
  'getYear',
  'setDate',
  'setFullYear',
  'setHours',
  'setMilliseconds',
  'setMinutes',
  'setMonth',
  'setSeconds',
  'toDateString',
  'toLocaleDateString',
  'toLocaleTimeString',
  'toTimeString',
]);

/** `Intl.DateTimeFormat`, as an expression. */
function isDateTimeFormat(node: Rule.Node | { type: string }): boolean {
  const member = node as {
    type: string;
    object?: { type: string; name?: string };
    property?: { type: string; name?: string };
  };
  return (
    member.type === 'MemberExpression' &&
    member.object?.type === 'Identifier' &&
    member.object.name === 'Intl' &&
    member.property?.type === 'Identifier' &&
    member.property.name === 'DateTimeFormat'
  );
}

/**
 * The logic must give the same result wherever it runs, so it may not ask
 * the device what time it is "here". The reader's time zone is an argument.
 *
 * Refused:
 * - the methods of `Date` that work in device time (`getHours`, `setDate`, ...);
 *   the `getUTC...` ones are fine
 * - `new Date(year, month, ...)`, which reads its numbers as device time, and
 *   `new Date('some text')`, which may
 * - `Intl.DateTimeFormat` without a `timeZone` option, which formats in device time
 * - reading the device's zone from `resolvedOptions().timeZone`
 */
const noDeviceTime: Rule.RuleModule = {
  meta: {
    type: 'problem',
    docs: { description: 'the logic does not use the time zone of the device' },
    schema: [],
    messages: {
      method:
        "`{{name}}` works in the time zone of the device. Take the reader's time zone as an argument (see lib/logic/dates.ts).",
      construct:
        'This `new Date(...)` reads its arguments as device time. Use `Date.UTC(...)` or the functions of lib/logic/dates.ts.',
      format:
        "`Intl.DateTimeFormat` without a `timeZone` option formats in the time zone of the device. Pass the reader's time zone.",
      zone: 'The time zone of the device is read in lib/regional.ts only. Take it as an argument.',
    },
  },
  create(context) {
    function checkFormat(
      node: Rule.Node,
      args: readonly { type: string; properties?: unknown[] }[],
    ): void {
      const options = args[1];
      const properties = (options?.type === 'ObjectExpression' ? options.properties : undefined) as
        { type: string; key?: { type: string; name?: string; value?: unknown } }[] | undefined;
      const named = properties?.some(
        (property) =>
          property.type === 'SpreadElement' ||
          (property.type === 'Property' &&
            (property.key?.name === 'timeZone' || property.key?.value === 'timeZone')),
      );
      // Options that are not written out here (a variable) are given the benefit of the doubt.
      if (options === undefined || (options.type === 'ObjectExpression' && !named)) {
        context.report({ node, messageId: 'format' });
      }
    }

    return {
      CallExpression(node) {
        const callee = node.callee;
        if (isDateTimeFormat(callee)) checkFormat(node, node.arguments);
        if (callee.type !== 'MemberExpression' || callee.property.type !== 'Identifier') return;
        if (DEVICE_TIME_METHODS.has(callee.property.name)) {
          context.report({ node, messageId: 'method', data: { name: callee.property.name } });
        }
      },
      NewExpression(node) {
        if (isDateTimeFormat(node.callee)) checkFormat(node, node.arguments);
        if (node.callee.type !== 'Identifier' || node.callee.name !== 'Date') return;
        const [first] = node.arguments;
        const text = first?.type === 'Literal' && typeof first.value === 'string';
        const template = first?.type === 'TemplateLiteral';
        if (node.arguments.length >= 2 || text || template) {
          context.report({ node, messageId: 'construct' });
        }
      },
      MemberExpression(node) {
        if (
          node.property.type === 'Identifier' &&
          node.property.name === 'timeZone' &&
          node.object.type === 'CallExpression' &&
          node.object.callee.type === 'MemberExpression' &&
          node.object.callee.property.type === 'Identifier' &&
          node.object.callee.property.name === 'resolvedOptions'
        ) {
          context.report({ node, messageId: 'zone' });
        }
      },
    };
  },
};

/**
 * The layers of src/lib, from the bottom. A file imports from its own layer
 * and from layers of a lower level; nothing else. The name of a layer is the
 * first folder under src/lib, or the name of a file there without its extension.
 *
 * `components` sits above `state` in the stack but is kept from it all the
 * same (`NEVER`): a shared component takes values and callbacks, so that it
 * can be drawn in a story or a test without a store behind it.
 */
export const LAYERS: Readonly<Record<string, number>> = {
  logic: 0,
  i18n: 0,
  hooks: 0,
  utils: 0,
  keys: 0,
  focus: 0,
  'theme.svelte': 0,
  'fragment-token': 0,
  save: 0,
  'chunk-reload.svelte': 1,
  regional: 1,
  'display.svelte': 1,
  fonts: 1,
  layout: 1,
  drag: 1,
  editor: 1,
  'file-viewer': 1,
  state: 2,
  ui: 2,
  components: 3,
  api: 3,
  features: 4,
};

/** Imports the levels would allow and the design does not. */
const NEVER: Readonly<Record<string, readonly string[]>> = {
  components: ['state'],
};

const layerOrder: Rule.RuleModule = {
  meta: {
    type: 'problem',
    docs: { description: 'a layer of src/lib imports only from the layers below it' },
    schema: [],
    messages: {
      unknown:
        'lib/{{layer}} is not a known layer. Add it to LAYERS in tools/lint/layers.ts and to src/lib/README.md.',
      upward:
        'lib/{{from}} imports from lib/{{to}}, which is not below it. See src/lib/README.md for the layers.',
      never:
        'lib/{{from}} does not import from lib/{{to}}: it takes values and callbacks instead. See src/lib/README.md.',
    },
  },
  create(context) {
    const from = place(context.filename);
    if (!from) return {};
    const fromLevel = LAYERS[from.layer];
    if (fromLevel === undefined) {
      return {
        Program(node) {
          context.report({ node, messageId: 'unknown', data: { layer: from.layer } });
        },
      };
    }

    function check(node: Rule.Node, source: unknown): void {
      const specifier =
        typeof source === 'object' && source !== null && 'value' in source
          ? source.value
          : undefined;
      if (typeof specifier !== 'string') return;
      const target = resolve(specifier, context.filename);
      const to = target === undefined ? undefined : place(target);
      if (!to || !from || to.layer === from.layer) return;
      const toLevel = LAYERS[to.layer];
      const data = { from: from.layer, to: to.layer };
      if (NEVER[from.layer]?.includes(to.layer)) {
        context.report({ node, messageId: 'never', data });
      } else if (toLevel === undefined || toLevel >= (fromLevel ?? 0)) {
        context.report({ node, messageId: 'upward', data });
      }
    }

    return {
      ImportDeclaration: (node) => {
        check(node, node.source);
      },
      ExportNamedDeclaration: (node) => {
        check(node, node.source);
      },
      ExportAllDeclaration: (node) => {
        check(node, node.source);
      },
      ImportExpression: (node) => {
        check(node, node.source);
      },
    };
  },
};

/** Globals that reach a server. */
const SERVER_GLOBALS = new Set(['fetch', 'XMLHttpRequest', 'WebSocket', 'EventSource']);
/** The folders under src/lib that are implementations of the data boundary. */
const SERVER_LAYERS = [`state${path.sep}memory`, 'api'];

/**
 * A screen reads and writes through `lib/state` and never talks to a server
 * itself: no `fetch`, no `XMLHttpRequest`, `WebSocket`, `EventSource` or
 * `sendBeacon`, and no import of an implementation of the boundary (the
 * in-memory one, or the one that will call the server). The one place that
 * chooses the implementation is exempted in eslint.config.js.
 */
const noDirectServer: Rule.RuleModule = {
  meta: {
    type: 'problem',
    docs: { description: 'screens reach data through lib/state only' },
    schema: [],
    messages: {
      global:
        '`{{name}}` reaches a server from a screen. Read and write through lib/state instead.',
      implementation:
        'This imports an implementation of the data boundary. Screens use the interface of lib/state; only src/app/backend.ts picks the implementation.',
    },
  },
  create(context) {
    function checkImport(node: Rule.Node, source: unknown): void {
      const specifier =
        typeof source === 'object' && source !== null && 'value' in source
          ? source.value
          : undefined;
      if (typeof specifier !== 'string') return;
      const target = resolve(specifier, context.filename);
      if (target === undefined) return;
      const at = target.lastIndexOf(LIB);
      if (at === -1) return;
      const inside = target.slice(at + LIB.length);
      if (SERVER_LAYERS.some((layer) => inside === layer || inside.startsWith(layer + path.sep))) {
        context.report({ node, messageId: 'implementation' });
      }
    }

    function nameOf(callee: {
      type: string;
      name?: string;
      property?: unknown;
    }): string | undefined {
      if (callee.type === 'Identifier') return callee.name;
      if (callee.type === 'MemberExpression') {
        const property = callee.property as { type: string; name?: string } | undefined;
        return property?.type === 'Identifier' ? property.name : undefined;
      }
      return undefined;
    }

    return {
      ImportDeclaration: (node) => {
        checkImport(node, node.source);
      },
      ExportNamedDeclaration: (node) => {
        checkImport(node, node.source);
      },
      ExportAllDeclaration: (node) => {
        checkImport(node, node.source);
      },
      ImportExpression: (node) => {
        checkImport(node, node.source);
      },
      CallExpression(node) {
        const name = nameOf(node.callee);
        if (name === 'fetch' || name === 'sendBeacon') {
          context.report({ node, messageId: 'global', data: { name } });
        }
      },
      NewExpression(node) {
        const name = nameOf(node.callee);
        if (name !== undefined && SERVER_GLOBALS.has(name)) {
          context.report({ node, messageId: 'global', data: { name } });
        }
      },
    };
  },
};

const plugin: ESLint.Plugin = {
  rules: {
    'feature-boundaries': featureBoundaries,
    'layer-order': layerOrder,
    'no-direct-server': noDirectServer,
    'no-device-time': noDeviceTime,
  },
};

export default plugin;
