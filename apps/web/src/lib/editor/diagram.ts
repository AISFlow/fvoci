/**
 * Draws diagrams with Mermaid.
 *
 * A diagram is a code block whose language is `mermaid`. Its text is written
 * by one user and drawn on the screen of another, so nothing about it is
 * trusted. There are four layers, and each is meant to hold on its own:
 *
 * 1. Before the library sees the text: a limit on its length, and a refusal
 *    of the one syntax that makes the library fetch a picture while it draws
 *    (a node drawn as an image).
 * 2. The library's settings (`CONFIG`): its strictest level that still gives
 *    a picture this page can check, labels as SVG text and never as HTML,
 *    and every setting it knows locked against the diagram's own text. A
 *    diagram may carry settings (a directive, front matter); the library
 *    drops the locked ones, which is all of them, and draws with ours.
 * 3. A setting that survives that is one the lock did not know, and nothing
 *    is known about what it does. The library is asked what settings are
 *    left once it has read the text (the same reading it uses when it
 *    draws), and a diagram with any left is not drawn at all. With the
 *    version in use nothing survives (it drops settings it does not know as
 *    well), so this is a tripwire for a later version, not a path in use.
 * 4. The picture the library returns is not put into the page. It is rebuilt
 *    from lists of allowed elements and attributes (safe-svg.ts).
 *
 * The library is fetched the first time a diagram has to be drawn.
 */
import type { MermaidConfig } from 'mermaid';

import { safeSvg } from './safe-svg.ts';
import { MAX_DIAGRAM_SOURCE } from './schema.ts';

/** How long a diagram may take before it is given up on, in milliseconds. */
export const DIAGRAM_TIMEOUT = 8000;

/**
 * The settings that do not depend on the screen.
 *
 * - `securityLevel: 'strict'`. The library has four levels (its
 *   docs/config/usage.md, "securityLevel", version 12.3.0). `loose` and
 *   `antiscript` allow HTML in labels and clicks. `sandbox` draws inside an
 *   `<iframe sandbox>` and hands back that frame, with the picture as a
 *   `data:` address: nothing in it can run, but neither can this page look
 *   inside, give it the app's colours, or filter it, and a frame from a
 *   `data:` address is one more thing a content policy would have to allow.
 *   `strict` encodes HTML in text and turns clicks off, and returns markup
 *   that layer 4 can check; of the levels that do, it is the strictest.
 * - `htmlLabels: false`: labels are SVG `<text>`, not HTML in
 *   `<foreignObject>` (which layer 4 drops in any case).
 * - `secure`: the settings a diagram's own text may not change. The library
 *   protects six by default; here it is every setting it knows (filled in
 *   when it is loaded) and the ones below that have no default.
 * - `suppressErrorRendering`: a diagram that cannot be read throws, and no
 *   picture of an error is left in the page.
 * - `maxTextSize`, `maxEdges`: the library's own limits, at or under its defaults.
 * - `layout: 'dagre'`: see below.
 * - `startOnLoad: false`: it draws only what it is handed.
 */
const CONFIG = {
  startOnLoad: false,
  securityLevel: 'strict',
  htmlLabels: false,
  suppressErrorRendering: true,
  maxTextSize: MAX_DIAGRAM_SOURCE,
  maxEdges: 500,
  logLevel: 'fatal',
  theme: 'base',
  // Laid out with the engine under a licence the project allows. The library's
  // default is ELK, whose package (EPL-2.0 or GPL-3.0) the build leaves out:
  // vite.config.ts puts a stub in its place and the notices step fails the
  // build should the real one ever be bundled.
  layout: 'dagre',
} as const satisfies MermaidConfig;

/** Settings without a default, which the list of defaults would miss. */
const ALSO_SECURE = [
  'htmlLabels',
  'themeCSS',
  'themeVariables',
  'theme',
  'fontFamily',
  'altFontFamily',
  'dompurifyConfig',
  'deterministicIDSeed',
  'look',
  'layout',
  'wrap',
];

/**
 * Which design token gives which colour of a diagram. The tokens are those
 * of src/styles/app.css; the names on the left are the library's theme
 * variables (its "base" theme works the rest out from these).
 */
const PALETTE: Record<string, string> = {
  background: '--background',
  primaryColor: '--muted',
  primaryTextColor: '--foreground',
  primaryBorderColor: '--input-border',
  secondaryColor: '--option-blue',
  secondaryTextColor: '--foreground',
  secondaryBorderColor: '--input-border',
  tertiaryColor: '--card',
  tertiaryTextColor: '--foreground',
  tertiaryBorderColor: '--border',
  lineColor: '--muted-foreground',
  textColor: '--foreground',
  titleColor: '--foreground',
  mainBkg: '--muted',
  nodeBorder: '--input-border',
  clusterBkg: '--card',
  clusterBorder: '--border',
  edgeLabelBackground: '--background',
  noteBkgColor: '--option-yellow',
  noteTextColor: '--foreground',
  noteBorderColor: '--input-border',
  activationBkgColor: '--option-blue',
  activationBorderColor: '--input-border',
  errorBkgColor: '--option-red',
  errorTextColor: '--foreground',
};
/** The colours of things that come in series: slices of a pie, branches, sections. */
const SERIES = [
  '--option-blue',
  '--option-green',
  '--option-yellow',
  '--option-orange',
  '--option-red',
  '--option-purple',
  '--option-pink',
  '--option-brown',
  '--option-gray',
  '--option-default',
  '--muted',
  '--secondary',
];

/** What can go wrong, for the caller to put into words. */
export type DiagramFailure = 'too-long' | 'not-allowed' | 'invalid' | 'timeout' | 'failed';

export type DiagramResult =
  { ok: true; picture: SVGSVGElement } | { ok: false; reason: DiagramFailure; detail?: string };

interface Library {
  initialize: (config: MermaidConfig) => void;
  parse: (text: string) => Promise<{ config?: unknown } | boolean | undefined>;
  render: (id: string, text: string, container?: Element) => Promise<{ svg: string }>;
  mermaidAPI: {
    defaultConfig: Record<string, unknown>;
    getDiagramFromText?: (text: string) => Promise<{ db?: unknown }>;
  };
}

let loading: Promise<Library> | undefined;
let configuredFor: string | undefined;
let queue: Promise<unknown> = Promise.resolve();
let serial = 0;

function loadLibrary(): Promise<Library> {
  loading ??= import('mermaid').then(
    (module) => module.default as unknown as Library,
    (error: unknown) => {
      loading = undefined;
      throw error;
    },
  );
  return loading;
}

class UnreadableColor extends Error {}

/**
 * A colour as `#rrggbb`. The tokens are written in a colour space the
 * library cannot read, so the browser is asked to paint one pixel and the
 * pixel is read back. A colour that is partly transparent is painted over
 * the background first.
 */
function hexFromParts(parts: readonly number[]): string {
  return `#${parts.map((part) => part.toString(16).padStart(2, '0')).join('')}`;
}

function hexOf(context: CanvasRenderingContext2D, color: string, under?: string): string {
  context.clearRect(0, 0, 1, 1);
  if (under !== undefined) {
    context.fillStyle = under;
    context.fillRect(0, 0, 1, 1);
  }
  // A colour the canvas cannot read leaves the one before it in place. The
  // one before is a marker that is never painted, not a colour of the page.
  const before = hexFromParts([1, 2, under === undefined ? 3 : 4]);
  context.fillStyle = before;
  context.fillStyle = color;
  if (context.fillStyle === before) throw new UnreadableColor();
  context.fillRect(0, 0, 1, 1);
  const [red = 0, green = 0, blue = 0] = context.getImageData(0, 0, 1, 1).data;
  return hexFromParts([red, green, blue]);
}

/**
 * The colours and the font of the page as it is now, in the library's
 * terms; undefined where they cannot be read (then the library's own neutral
 * theme is used, which follows nothing).
 */
function themeOfPage(page: Document): Record<string, string | boolean> | undefined {
  const context = page.createElement('canvas').getContext('2d', { willReadFrequently: true });
  const view = page.defaultView;
  if (!context || !view) return undefined;
  const probe = page.createElement('span');
  probe.hidden = true;
  page.body.append(probe);
  try {
    const computed = (token: string): string => {
      probe.style.color = `var(${token})`;
      return view.getComputedStyle(probe).color;
    };
    const background = hexOf(context, computed('--background'));
    const hex = (token: string): string => hexOf(context, computed(token), background);
    const variables: Record<string, string | boolean> = {};
    for (const [name, token] of Object.entries(PALETTE)) variables[name] = hex(token);
    SERIES.forEach((token, index) => {
      const color = hex(token);
      variables[`pie${index + 1}`] = color;
      variables[`cScale${index}`] = color;
      variables[`cScaleLabel${index}`] = hex('--foreground');
      if (index < 8) {
        variables[`git${index}`] = color;
        variables[`gitBranchLabel${index}`] = hex('--foreground');
        variables[`gitInv${index}`] = hex('--foreground');
      }
    });
    variables.pieTitleTextColor = hex('--foreground');
    variables.pieSectionTextColor = hex('--foreground');
    variables.pieLegendTextColor = hex('--foreground');
    variables.pieStrokeColor = background;
    variables.pieOuterStrokeColor = hex('--input-border');
    variables.commitLabelColor = hex('--foreground');
    variables.commitLabelBackground = hex('--muted');
    variables.tagLabelColor = hex('--foreground');
    variables.tagLabelBackground = hex('--muted');
    variables.tagLabelBorder = hex('--input-border');
    // Dark when the background is: the library lightens or darkens what it derives accordingly.
    const [red, green, blue] = [1, 3, 5].map((at) =>
      Number.parseInt(background.slice(at, at + 2), 16),
    );
    variables.darkMode = 0.2126 * (red ?? 0) + 0.7152 * (green ?? 0) + 0.0722 * (blue ?? 0) < 128;
    variables.fontFamily = view.getComputedStyle(page.body).fontFamily;
    return variables;
  } catch (error) {
    if (error instanceof UnreadableColor) return undefined;
    throw error;
  } finally {
    probe.remove();
  }
}

/** Sets the library up for the page as it looks now; again only when the look has changed. */
function configure(library: Library, page: Document): void {
  const themeVariables = themeOfPage(page);
  const signature = JSON.stringify(themeVariables ?? null);
  if (signature === configuredFor) return;
  configuredFor = signature;
  library.initialize({
    ...CONFIG,
    ...(themeVariables ? { themeVariables } : { theme: 'neutral' }),
    secure: [...new Set([...Object.keys(library.mermaidAPI.defaultConfig), ...ALSO_SECURE])],
  });
}

/**
 * A node drawn as a picture makes the library fetch that picture while it
 * draws, before anything here could filter it. The syntax is a block of
 * node data, `@{ … }`, with an `img` key. That block is YAML, where a key
 * can also be spelled with escapes, so a diagram that uses node data is
 * refused when "img" or a backslash occurs anywhere in it.
 */
function fetchesWhileDrawing(source: string): boolean {
  return source.includes('@{') && (/img/i.test(source) || source.includes('\\'));
}

/** The same question, asked of what the library read: does any node of a flowchart name a picture? */
async function namesAPicture(library: Library, source: string): Promise<boolean> {
  const diagram = await library.mermaidAPI.getDiagramFromText?.(source);
  const database = diagram?.db as { getVertices?: () => unknown } | undefined;
  const vertices = database?.getVertices?.();
  if (!(vertices instanceof Map)) return false;
  return [...vertices.values()].some(
    (vertex) =>
      typeof vertex === 'object' && vertex !== null && Boolean((vertex as { img?: unknown }).img),
  );
}

const messageOf = (error: unknown): string =>
  (error instanceof Error ? error.message : String(error)).slice(0, 600);

async function draw(source: string, width: number, page: Document): Promise<DiagramResult> {
  if (source.length > MAX_DIAGRAM_SOURCE) return { ok: false, reason: 'too-long' };
  if (fetchesWhileDrawing(source)) return { ok: false, reason: 'not-allowed' };

  const library = await loadLibrary();
  configure(library, page);

  let held: unknown;
  try {
    const parsed = await library.parse(source);
    held = typeof parsed === 'object' ? parsed.config : undefined;
    if (await namesAPicture(library, source)) return { ok: false, reason: 'not-allowed' };
  } catch (error) {
    return { ok: false, reason: 'invalid', detail: messageOf(error) };
  }
  // The library has dropped the settings it knows from what the text carried
  // (all of them are locked). One that is still there is one nobody vouches for.
  if (typeof held === 'object' && held !== null && Object.keys(held).length > 0) {
    return { ok: false, reason: 'not-allowed' };
  }

  // The library draws in the page to measure text. It gets a place of its
  // own for that, out of sight (editor.css), as wide as the picture will be.
  const stage = page.createElement('div');
  stage.setAttribute('class', 'doc-diagram-stage');
  stage.setAttribute('aria-hidden', 'true');
  stage.style.width = `${Math.max(240, Math.round(width))}px`;
  page.body.append(stage);
  serial += 1;
  const diagramId = `doc-diagram-${serial}`;
  try {
    const { svg } = await library.render(diagramId, source, stage);
    const picture = safeSvg(svg, { document: page, expectedId: diagramId });
    return picture ? { ok: true, picture } : { ok: false, reason: 'failed' };
  } catch (error) {
    return { ok: false, reason: 'failed', detail: messageOf(error) };
  } finally {
    stage.remove();
  }
}

/**
 * Draws a diagram: the picture, or why there is none. Never throws.
 *
 * Diagrams are drawn one after the other (the library keeps its settings in
 * one place). `width` is how wide the picture may become, which a few kinds
 * of diagram lay themselves out for. A diagram that takes longer than
 * `DIAGRAM_TIMEOUT` is given up on; the library cannot be interrupted, so
 * the limit is on waiting, and the next diagram waits for this one to end.
 */
export function drawDiagram(
  source: string,
  width: number,
  page: Document = document,
): Promise<DiagramResult> {
  const turn = queue;
  const work = turn
    .then(() => draw(source, width, page))
    .catch((error: unknown): DiagramResult => ({
      ok: false,
      reason: 'failed',
      detail: messageOf(error),
    }));
  // What comes next waits for the work itself, not for the limit on waiting.
  queue = work;
  return turn.then(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const limit = new Promise<DiagramResult>((resolve) => {
      timer = setTimeout(() => {
        resolve({ ok: false, reason: 'timeout' });
      }, DIAGRAM_TIMEOUT);
    });
    return Promise.race([work, limit]).finally(() => {
      clearTimeout(timer);
    });
  });
}
