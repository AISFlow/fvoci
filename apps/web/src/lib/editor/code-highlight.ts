/**
 * The colours and the line numbers of the code blocks in the editor, as
 * decorations: nothing of them is in the document, which stays exactly what
 * it was (no mark, no node, no attribute), and so does what is stored and
 * what is sent to the others who edit it.
 *
 * Each code block in a known language is coloured from the tokens of its
 * text (code-highlighter.ts), which come from a worker a moment after the
 * text changes. Until they come, the tokens of the block's last text are
 * moved onto the new one (`moveTokens`): what was typed is plain for that
 * moment and the rest keeps its colour. That holds for a change from another
 * reader too, which the shared document brings as a new block.
 *
 * While an input method composes (Hangul, kana), the decorations are only
 * carried along with the text and nothing new is drawn around the syllable
 * being composed: drawing would end the composition. They are brought up to
 * date once it ends. Whatever is drawn at a moment of the plugin's own reads
 * the page's caret first, which a key may have moved a moment before
 * (moves-kept.ts): drawing would put it back.
 *
 * The numbers of the lines are widgets at the start of each line, drawn as
 * where a body is only shown (code-paint.ts), while the reader has asked for
 * them (code-lines.ts); a block too long for that has them in one
 * column, which its view draws (`numberedInColumn`).
 */
import type { Node as DocumentNode } from '@tiptap/pm/model';
import { Plugin, PluginKey, type Transaction } from '@tiptap/pm/state';
import { Decoration, DecorationSet, type EditorView } from '@tiptap/pm/view';

import { noTypingFromControls } from './browser-edits.ts';
import { CODE_CONTROLS, codeBlocksEditable } from './code-block-view.ts';
import { knownTokens, tokensFor } from './code-highlighter.ts';
import { codeLanguageOf, type CodeLanguage } from './code-languages.ts';
import { codeLinesShown, onCodeLinesChange } from './code-lines.ts';
import { lineMarker, lineStarts, numberedInColumn } from './code-paint.ts';
import { readPage } from './moves-kept.ts';
import { moveTokens, tokenClassAt, type CodeTokens } from './code-tokens.ts';
import { DIAGRAM_LANGUAGE } from './schema.ts';

/** A code block as it was last decorated. */
interface Block {
  pos: number;
  node: DocumentNode;
  language: CodeLanguage | undefined;
  text: string;
  tokens: CodeTokens;
  /** The tokens are those of `text`, or the block stays plain; otherwise they are moved from an older text. */
  fresh: boolean;
}

interface HighlightState {
  blocks: readonly Block[];
  decorations: DecorationSet;
  /** Whether the numbers of the lines are drawn. */
  lines: boolean;
}

/** What a transaction may tell the plugin: the numbers were turned on or off, or new tokens came. */
interface HighlightMeta {
  lines?: boolean;
  refresh?: true;
}

export const codeHighlightKey = new PluginKey<HighlightState>('codeHighlight');

const NONE: CodeTokens = [];

/** Each code block of a document that is no diagram, with where it starts. */
function eachCodeBlock(doc: DocumentNode, visit: (node: DocumentNode, pos: number) => void): void {
  doc.descendants((node, pos) => {
    if (node.type.name === 'codeBlock') {
      if (node.attrs.language !== DIAGRAM_LANGUAGE) visit(node, pos);
      return false;
    }
    // A code block is never inside another block's text.
    return !node.isTextblock;
  });
}

/** The block at `pos` now, from what the block was before (`before`) if anything is known of it. */
function blockOf(node: DocumentNode, pos: number, before: Block | undefined): Block {
  if (before?.node === node && before.fresh) return { ...before, pos };
  const language = codeLanguageOf(node.attrs.language);
  const text = node.textContent;
  if (!language) return { pos, node, language, text, tokens: NONE, fresh: true };
  const known = knownTokens(language, text);
  if (known !== undefined) return { pos, node, language, text, tokens: known ?? NONE, fresh: true };
  const moved =
    before && before.language?.grammar === language.grammar
      ? moveTokens(before.tokens, before.text, text)
      : NONE;
  return { pos, node, language, text, tokens: moved, fresh: false };
}

/** What a decoration of a block is: a run of a colour (`kind`) or the number of a line (`line`). */
interface CodeDecorationSpec {
  kind?: number;
  line?: true;
}

/** Draws the number of a line. Every one is the same (code-paint.ts), and so is redrawn never. */
const lineWidget = (view: EditorView): HTMLElement => lineMarker(view.dom.ownerDocument);

/**
 * The decorations of one block, in the order of a decoration set: by where
 * they start, a line's number before a run that starts with the line.
 */
function decorationsOf(block: Block, lines: boolean): Decoration[] {
  const start = block.pos + 1;
  const made: Decoration[] = [];
  const { tokens } = block;
  const all = lines ? lineStarts(block.text) : [];
  // A block too long for a number at each line has a column of them instead (code-block-view.ts).
  const starts = numberedInColumn(all.length, block.text.length) ? [] : all;
  let line = 0;
  const numberUpTo = (at: number): void => {
    for (; line < starts.length && (starts[line] ?? 0) <= at; line += 1) {
      const spec: CodeDecorationSpec = { line: true };
      made.push(
        Decoration.widget(start + (starts[line] ?? 0), lineWidget, {
          side: -1,
          key: 'code-line',
          ignoreSelection: true,
          marks: [],
          ...spec,
        }),
      );
    }
  };
  for (let index = 0; index + 2 < tokens.length; index += 3) {
    const kind = tokens[index + 2] ?? -1;
    const className = tokenClassAt(kind);
    if (className === undefined) continue;
    const from = tokens[index] ?? 0;
    numberUpTo(from);
    // What is typed at the end of a run takes its colour until the new tokens come.
    const spec: CodeDecorationSpec = { kind };
    made.push(
      Decoration.inline(
        start + from,
        start + (tokens[index + 1] ?? 0),
        { class: className },
        {
          inclusiveEnd: true,
          ...spec,
        },
      ),
    );
  }
  numberUpTo(Number.POSITIVE_INFINITY);
  return made;
}

const specOf = (decoration: Decoration): CodeDecorationSpec =>
  decoration.spec as CodeDecorationSpec;

/** Whether two decorations of a block are the same: in the same place, of the same kind. */
const same = (a: Decoration, b: Decoration): boolean =>
  a.from === b.from &&
  a.to === b.to &&
  specOf(a).kind === specOf(b).kind &&
  specOf(a).line === specOf(b).line;

/** Which comes first in a decoration set. */
const before = (a: Decoration, b: Decoration): boolean =>
  a.from < b.from || (a.from === b.from && a.to < b.to);

/**
 * What changes between the decorations a block had (carried along with the
 * document) and those it has now, both in their order: what is the same in
 * both is kept, which is most of a long block when a few letters changed.
 */
function difference(
  had: readonly Decoration[],
  has: readonly Decoration[],
  removed: Decoration[],
  added: Decoration[],
): void {
  let old = 0;
  let now = 0;
  for (let a = had[old], b = has[now]; a || b; a = had[old], b = has[now]) {
    if (a && b && same(a, b)) {
      old += 1;
      now += 1;
    } else if (a && (!b || before(a, b))) {
      removed.push(a);
      old += 1;
    } else if (b && (!a || before(b, a))) {
      added.push(b);
      now += 1;
    } else if (a && b) {
      // In the same place, of another kind.
      removed.push(a);
      added.push(b);
      old += 1;
      now += 1;
    }
  }
}

/**
 * Taking decorations out of a set one by one costs a look through the
 * block's others for each; past this many, the set is made anew instead.
 */
const REMOVED_ONE_BY_ONE = 500;

/**
 * The state after a change: each block whose text, language or place changed
 * (or all of them, when the numbers were turned on or off) is decorated anew,
 * and only what differs from what it had is changed in the set; the
 * decorations of the others are carried along with the document.
 */
function settle(
  previous: HighlightState,
  doc: DocumentNode,
  tr: Transaction | undefined,
  lines: boolean,
): HighlightState {
  const mapping = tr?.docChanged ? tr.mapping : undefined;
  const carried = mapping ? previous.decorations.map(mapping, doc) : previous.decorations;
  /** The blocks whose start is still there, by where it is now; and those that were replaced, by where they were. */
  const kept = new Map<number, Block>();
  const replaced = new Map<number, Block>();
  for (const block of previous.blocks) {
    const moved = mapping?.mapResult(block.pos, 1);
    (moved?.deleted ? replaced : kept).set(moved?.pos ?? block.pos, block);
  }
  const blocks: Block[] = [];
  const removed: Decoration[] = [];
  const added: Decoration[] = [];
  /** Each block's decorations now, or `undefined` for one that keeps what it had. */
  const drawn: (Decoration[] | undefined)[] = [];
  eachCodeBlock(doc, (node, pos) => {
    const old = kept.get(pos);
    kept.delete(pos);
    const block = blockOf(node, pos, old ?? replaced.get(pos));
    blocks.push(block);
    if (old?.node === node && old.tokens === block.tokens && lines === previous.lines) {
      drawn.push(undefined);
      return;
    }
    const has = decorationsOf(block, lines);
    drawn.push(has);
    difference(carried.find(pos + 1, pos + 1 + node.content.size), has, removed, added);
  });
  // A block that is no code block any more (a paragraph now, or a diagram) keeps its text: not its colours.
  for (const pos of kept.keys()) {
    const node = doc.nodeAt(pos);
    for (const decoration of node ? carried.find(pos + 1, pos + node.nodeSize - 1) : [])
      removed.push(decoration);
  }
  if (removed.length === 0 && added.length === 0) return { blocks, decorations: carried, lines };
  if (removed.length <= REMOVED_ONE_BY_ONE)
    return { blocks, decorations: carried.remove(removed).add(doc, added), lines };
  const all: Decoration[] = [];
  blocks.forEach((block, index) => {
    const end = block.pos + 1 + block.node.content.size;
    for (const decoration of drawn[index] ?? carried.find(block.pos + 1, end)) all.push(decoration);
  });
  return { blocks, decorations: DecorationSet.create(doc, all), lines };
}

/** The state while an input method composes: carried along with the text, nothing drawn anew. */
function carry(previous: HighlightState, tr: Transaction): HighlightState {
  const blocks = previous.blocks.flatMap((block) => {
    const moved = tr.mapping.mapResult(block.pos, 1);
    return moved.deleted ? [] : [{ ...block, pos: moved.pos }];
  });
  return { ...previous, blocks, decorations: previous.decorations.map(tr.mapping, tr.doc) };
}

/** Asks for the tokens the blocks lack, and has the editor take them up as they come. */
function askForTokens(view: EditorView, composing: () => boolean) {
  const asked = new Map<string, AbortController>();
  let destroyed = false;
  let waiting = false;
  const refresh = (): void => {
    if (waiting || destroyed) return;
    waiting = true;
    setTimeout(() => {
      waiting = false;
      if (destroyed || composing()) return;
      readPage(view);
      view.dispatch(view.state.tr.setMeta(codeHighlightKey, { refresh: true }));
    }, 0);
  };
  const ask = (): void => {
    const wanted = new Set<string>();
    for (const block of codeHighlightKey.getState(view.state)?.blocks ?? []) {
      if (block.fresh || !block.language) continue;
      const { language, text } = block;
      const key = `${language.grammar}\n${text}`;
      wanted.add(key);
      if (asked.has(key)) continue;
      const controller = new AbortController();
      asked.set(key, controller);
      void tokensFor(language, text, controller.signal).then(() => {
        if (asked.get(key) === controller) asked.delete(key);
        if (!controller.signal.aborted) refresh();
      });
    }
    // What was asked for a text the block no longer has is given up, if it has not started.
    for (const [key, controller] of asked) {
      if (wanted.has(key)) continue;
      controller.abort();
      asked.delete(key);
    }
  };
  return {
    ask,
    refresh,
    destroy: () => {
      destroyed = true;
      for (const controller of asked.values()) controller.abort();
    },
  };
}

/** The plugin, one for each editor. */
export function codeHighlight(): Plugin<HighlightState> {
  let composing = false;
  let tokens: ReturnType<typeof askForTokens> | undefined;
  return new Plugin<HighlightState>({
    key: codeHighlightKey,
    state: {
      init: (_config, state) => {
        const shown = typeof document !== 'undefined' && codeLinesShown(document);
        const empty = { blocks: [], decorations: DecorationSet.empty, lines: shown };
        return settle(empty, state.doc, undefined, shown);
      },
      apply: (tr, previous) => {
        const meta = tr.getMeta(codeHighlightKey) as HighlightMeta | undefined;
        if (!tr.docChanged && !meta) return previous;
        if (composing && !meta) return carry(previous, tr);
        return settle(previous, tr.doc, tr, meta?.lines ?? previous.lines);
      },
    },
    props: {
      decorations: (state) => codeHighlightKey.getState(state)?.decorations,
      handleDOMEvents: {
        compositionstart: () => {
          composing = true;
          return false;
        },
        compositionend: () => {
          composing = false;
          tokens?.refresh();
          return false;
        },
        beforeinput: noTypingFromControls(CODE_CONTROLS),
      },
    },
    view: (view) => {
      const page = view.dom.ownerDocument;
      const asking = askForTokens(view, () => composing);
      tokens = asking;
      asking.ask();
      const stopWatching = onCodeLinesChange(page, (lines) => {
        readPage(view);
        view.dispatch(view.state.tr.setMeta(codeHighlightKey, { lines }));
      });
      // The editor may be told it is read-only only after its blocks are drawn.
      let editable = view.editable;
      codeBlocksEditable(view);
      return {
        update: () => {
          asking.ask();
          if (view.editable === editable) return;
          editable = view.editable;
          codeBlocksEditable(view);
        },
        destroy: () => {
          asking.destroy();
          stopWatching();
        },
      };
    },
  });
}
