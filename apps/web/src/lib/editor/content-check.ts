/**
 * Incremental ProseMirror schema check for collaborative bodies.
 *
 * `@tiptap/extension-collaboration` with `enableContentCheck: true` runs
 * `transaction.doc.check()` over the entire document on every remote Yjs
 * transaction, which walks all 100,000+ nodes for every incoming character.
 *
 * `CheckedCollaboration` keeps the exact same safety invariant:
 * - `ySyncPlugin` receives `onInvalidContent`, so unknown nodes or marks
 *   written by a newer version of the app are kept in the shared Y.Doc
 *   (never deleted for everyone) and editing pauses until the document fits.
 * - On the first shared document sync (and whenever recovering from an
 *   invalid state), the entire document is validated with `doc.check()`.
 * - On subsequent shared transactions, only the top-level blocks touched by
 *   `tr.steps` (mapped forward through `tr.mapping`) are checked with
 *   `child.check()`, together with the root `doc` node's own constraints
 *   (empty-document rule, allowed top-level child types and marks, and
 *   document attributes).
 */
import { Collaboration } from '@tiptap/extension-collaboration';
import type { Node as ProseMirrorNode, NodeType } from '@tiptap/pm/model';
import { Plugin, PluginKey, type EditorState, type Transaction } from '@tiptap/pm/state';
import {
  AddMarkStep,
  AddNodeMarkStep,
  AttrStep,
  DocAttrStep,
  RemoveMarkStep,
  RemoveNodeMarkStep,
  ReplaceAroundStep,
  ReplaceStep,
} from '@tiptap/pm/transform';
import { ySyncPluginKey } from '@tiptap/y-tiptap';

import { checkSubtreeAttrs } from './content-check-attrs.ts';

interface ContentCheckTracker {
  /** Whether at least one full `doc.check()` has passed on the current document. */
  checkedFull: boolean;
  /** Schema error caught by this plugin that paused editing, or `null` when valid. */
  error: Error | null;
}

export interface ContentCheckState {
  /** Mutable per-plugin tracker so rejected transactions still expose their error synchronously. */
  readonly tracker: ContentCheckTracker;
}

export const contentCheckPluginKey = new PluginKey<ContentCheckState>('bodyContentCheck');

/** Cache of whether a top node type accepts any sequence of a fixed set of child types. */
const uniformContentCache = new WeakMap<NodeType, boolean>();

function toError(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}

/**
 * Whether `type`'s content expression accepts any sequence of a fixed set of
 * direct child types (such as `block+`, `block*`, or `(block | documentBlock)+`).
 *
 * When true, a non-empty document matches `type.contentMatch` if and only if
 * every direct child individually matches an outgoing edge of `type.contentMatch`.
 * An empty document matches if and only if `type.contentMatch.validEnd` is true.
 */
function isUniformContent(type: NodeType): boolean {
  const cached = uniformContentCache.get(type);
  if (cached !== undefined) return cached;
  const start = type.contentMatch;
  let uniform = false;
  if (start.edgeCount > 0) {
    const after = start.edge(0).next;
    if (after.validEnd && after.edgeCount === start.edgeCount) {
      uniform = true;
      for (let i = 0; i < start.edgeCount; i += 1) {
        const edge = start.edge(i);
        const nextEdge = after.edge(i);
        if (edge.next !== after || nextEdge.type !== edge.type || nextEdge.next !== after) {
          uniform = false;
          break;
        }
      }
    }
  }
  uniformContentCache.set(type, uniform);
  return uniform;
}

interface Range {
  from: number;
  to: number;
}

/**
 * Maps each step's touched positions forward into `tr.doc` coordinates.
 * Returns `'full'` if an unrecognized step has an empty `StepMap`, so callers
 * safely fall back to a full `doc.check()`.
 */
function changedRangesOf(tr: Transaction): { ranges: Range[]; docAttrsChanged: boolean } | 'full' {
  const ranges: Range[] = [];
  let docAttrsChanged = tr.before.attrs !== tr.doc.attrs;

  const pushMapped = (stepIndex: number, from: number, to: number): void => {
    const tail = tr.mapping.slice(stepIndex + 1);
    const mappedFrom = tail.map(from, -1);
    const mappedTo = tail.map(to, 1);
    ranges.push({
      from: Math.min(mappedFrom, mappedTo),
      to: Math.max(mappedFrom, mappedTo),
    });
  };

  for (let i = 0; i < tr.steps.length; i += 1) {
    const step = tr.steps[i];
    if (step instanceof ReplaceAroundStep) {
      // Cover the entire post-step span including the preserved gap (e.g. unwrapping a container).
      pushMapped(i, step.from, step.from + step.slice.size + (step.gapTo - step.gapFrom));
      continue;
    }

    const beforeCount = ranges.length;
    tr.mapping.maps[i]?.forEach((_oldStart, _oldEnd, newStart, newEnd) => {
      pushMapped(i, newStart, newEnd);
    });
    if (ranges.length > beforeCount) continue;

    if (step instanceof AddMarkStep || step instanceof RemoveMarkStep) {
      pushMapped(i, step.from, step.to);
    } else if (step instanceof ReplaceStep) {
      pushMapped(i, step.from, step.from + step.slice.size);
    } else if (
      step instanceof AddNodeMarkStep ||
      step instanceof RemoveNodeMarkStep ||
      step instanceof AttrStep
    ) {
      pushMapped(i, step.pos, step.pos + 1);
    } else if (step instanceof DocAttrStep) {
      docAttrsChanged = true;
    } else {
      return 'full';
    }
  }
  return { ranges, docAttrsChanged };
}

/**
 * Validates the root `doc` node's own attributes without walking its children.
 * Returns `false` if a full `doc.check()` is needed instead.
 */
function checkDocAttrs(doc: ProseMirrorNode): boolean {
  const allowed = doc.type.spec.attrs ?? {};
  for (const key of Object.keys(doc.attrs)) {
    if (!Object.hasOwn(allowed, key)) {
      throw new RangeError(`Unsupported attribute ${key} for node of type ${doc.type.name}`);
    }
  }
  // `createAndFill` substitutes defaults for `undefined` attribute values via `computeAttrs`,
  // which would skip `validate(undefined)`. Fall back to `doc.check()` in that case.
  for (const key of Object.keys(allowed)) {
    if (doc.attrs[key] === undefined) return false;
  }
  const probe = doc.type.createAndFill(doc.attrs);
  if (!probe) return false;
  probe.check();
  return true;
}

/** Converts position ranges into sorted, non-overlapping top-level child index intervals `[from, to)`. */
function childIndexRanges(doc: ProseMirrorNode, ranges: readonly Range[]): Range[] {
  if (doc.childCount === 0 || ranges.length === 0) return [];
  const size = doc.content.size;
  const intervals: Range[] = [];
  for (const range of ranges) {
    const from = Math.max(0, Math.min(range.from, size));
    const to = Math.max(from, Math.min(range.to, size));
    const start = doc.resolve(from).index(0);
    const end = doc.resolve(to).indexAfter(0);
    if (start < end) intervals.push({ from: start, to: end });
  }
  if (intervals.length <= 1) return intervals;
  intervals.sort((a, b) => a.from - b.from || a.to - b.to);
  const merged: Range[] = [];
  for (const next of intervals) {
    const last = merged.at(-1);
    if (last && next.from <= last.to) last.to = Math.max(last.to, next.to);
    else merged.push({ from: next.from, to: next.to });
  }
  return merged;
}

/**
 * Checks that `tr.doc` conforms to its schema.
 *
 * When `full` is true, runs `tr.doc.check()` over the entire document.
 * Otherwise validates root `doc` constraints and calls `child.check()` only on
 * the top-level children touched by `tr.steps`.
 */
export function checkChangedContent(tr: Transaction, options?: { full?: boolean }): void {
  const { doc } = tr;
  if (options?.full || doc.marks.length > 0) {
    doc.check();
    checkSubtreeAttrs(doc);
    return;
  }

  const collected = changedRangesOf(tr);
  if (collected === 'full') {
    doc.check();
    checkSubtreeAttrs(doc);
    return;
  }

  if (collected.docAttrsChanged && !checkDocAttrs(doc)) {
    doc.check();
    checkSubtreeAttrs(doc);
    return;
  }

  const uniform = isUniformContent(doc.type);
  if (!uniform) {
    if (!doc.type.validContent(doc.content)) {
      throw new RangeError(
        `Invalid content for node ${doc.type.name}: ${doc.content.toString().slice(0, 50)}`,
      );
    }
  } else if (doc.childCount === 0) {
    if (!doc.type.contentMatch.validEnd) {
      throw new RangeError(`Invalid content for node ${doc.type.name}: <>`);
    }
    return;
  }

  for (const { from, to } of childIndexRanges(doc, collected.ranges)) {
    for (let index = from; index < to; index += 1) {
      const child = doc.child(index);
      if (
        uniform &&
        (doc.type.contentMatch.matchType(child.type) === null || !doc.type.allowsMarks(child.marks))
      ) {
        throw new RangeError(
          `Invalid content for node ${doc.type.name}: ${child.toString().slice(0, 50)}`,
        );
      }
      child.check();
      checkSubtreeAttrs(child);
    }
  }
}

/** Whether `state` is paused on content the editor's schema cannot draw. */
export function hasInvalidContent(state: EditorState): boolean {
  const syncError = ySyncPluginKey.getState(state)?.binding.invalidContentError ?? null;
  if (syncError !== null) return true;
  return (contentCheckPluginKey.getState(state)?.tracker.error ?? null) !== null;
}

export interface ContentCheckPluginOptions {
  /** Called once when content transitions from valid to invalid. */
  onError: (error: Error) => void;
  /** Called when a checked document is valid (for resetting shared error-reporting state). */
  onValid?: () => void;
  /** Returns true when collaboration has been explicitly disabled. */
  isDisabled?: () => boolean;
  /** Check only Yjs-originated transactions (default `true`). */
  onlyShared?: boolean;
}

/**
 * Whether `tr` carries a Yjs projection from `ySyncPlugin` (as opposed to a
 * metadata-only snapshot request like `renderSnapshot` which leaves `tr.doc`
 * at the last valid state).
 */
function isYjsProjectionTransaction(tr: Transaction): boolean {
  const meta = tr.getMeta(ySyncPluginKey) as
    { isChangeOrigin?: boolean; snapshot?: unknown; prevSnapshot?: unknown } | undefined;
  if (!meta) return false;
  if (meta.isChangeOrigin === true) return true;
  // `unrenderSnapshot` clears snapshots and projects the live Y.Doc without `isChangeOrigin`.
  return meta.snapshot === null && meta.prevSnapshot === null;
}

/**
 * ProseMirror plugin that runs a full `doc.check()` on initial load (and after
 * any invalid state) and checks only changed top-level blocks on subsequent
 * transactions.
 */
export function contentCheckPlugin(options: ContentCheckPluginOptions): Plugin<ContentCheckState> {
  const tracker: ContentCheckTracker = { checkedFull: false, error: null };
  const onlyShared = options.onlyShared ?? true;

  return new Plugin<ContentCheckState>({
    key: contentCheckPluginKey,
    state: {
      init: () => ({ tracker }),
      apply: (_tr, pluginState) => pluginState,
    },
    props: {
      editable: () => tracker.error === null,
    },
    filterTransaction: (tr, state) => {
      const syncError = ySyncPluginKey.getState(state)?.binding.invalidContentError ?? null;

      if (onlyShared && !isYjsProjectionTransaction(tr)) {
        if (tr.docChanged && (tracker.error !== null || syncError !== null)) return false;
        return true;
      }

      if (options.isDisabled?.()) return false;

      if (syncError !== null) {
        // `ySyncPlugin` already paused editing and notifies `onInvalidContent`;
        // force a full check once its projection error clears.
        tracker.error = null;
        tracker.checkedFull = false;
        return true;
      }

      const full = !tracker.checkedFull || tracker.error !== null;
      if (!tr.docChanged && !full) return true;

      try {
        checkChangedContent(tr, { full });
        tracker.checkedFull = true;
        tracker.error = null;
        options.onValid?.();
        return true;
      } catch (caught) {
        const error = toError(caught);
        const isNew = tracker.error === null;
        tracker.error = error;
        tracker.checkedFull = false;
        if (isNew) options.onError(error);
        return false;
      }
    },
  });
}

/**
 * `Collaboration` with initial full-document checking and incremental
 * changed-range checking on subsequent remote Yjs updates.
 *
 * Replaces `Editor`'s `enableContentCheck: true` on collaborative editors while
 * keeping `ySyncPlugin`'s `onInvalidContent` projection guard intact and
 * pausing shared writes (`ySyncPlugin` view updates and undo/redo) whenever
 * either `ySyncPlugin` or `contentCheckPlugin` holds a schema error.
 */
export const CheckedCollaboration = Collaboration.extend({
  addCommands() {
    const parent = this.parent?.();
    return {
      undo: () => (props) => {
        if (hasInvalidContent(props.state)) return false;
        return parent?.undo?.()(props) ?? false;
      },
      redo: () => (props) => {
        if (hasInvalidContent(props.state)) return false;
        return parent?.redo?.()(props) ?? false;
      },
    };
  },

  addProseMirrorPlugins() {
    const fragment =
      this.options.fragment ?? this.options.document?.getXmlFragment(this.options.field);
    const disableCollaboration = (): void => {
      this.storage.isDisabled = true;
      fragment?.doc?.destroy();
    };
    let reportedError = false;
    const emitError = (error: Error): void => {
      if (reportedError) return;
      reportedError = true;
      this.editor.emit('contentError', {
        error,
        editor: this.editor,
        disableCollaboration,
      });
    };

    const previousSyncOptions = this.options.ySyncOptions;
    const previousInvalidContent = previousSyncOptions?.onInvalidContent;
    const previousContentCheck = this.editor.options.enableContentCheck;
    let parentPlugins: Plugin[];
    try {
      this.options.ySyncOptions = {
        ...previousSyncOptions,
        onInvalidContent: (error: Error) => {
          previousInvalidContent?.(error);
          emitError(error);
        },
      };
      // Prevent Collaboration from attaching its full-document `filterInvalidContent` plugin
      // or overwriting our `onInvalidContent` callback if `enableContentCheck` was set.
      this.editor.options.enableContentCheck = false;
      parentPlugins = this.parent?.() ?? [];
    } finally {
      this.options.ySyncOptions = previousSyncOptions;
      this.editor.options.enableContentCheck = previousContentCheck;
    }

    const plugins = parentPlugins.map((plugin) => {
      if (plugin.spec.key !== ySyncPluginKey) return plugin;
      const originalView = plugin.spec.view;
      if (!originalView) return plugin;
      return new Plugin({
        ...plugin.spec,
        view: (view) => {
          const instance = originalView(view);
          return {
            ...instance,
            update: (currentView, prevState) => {
              if (hasInvalidContent(currentView.state)) return;
              instance.update?.(currentView, prevState);
            },
          };
        },
      });
    });

    return [
      ...plugins,
      contentCheckPlugin({
        onError: emitError,
        onValid: () => {
          reportedError = false;
        },
        isDisabled: () => this.storage.isDisabled,
        onlyShared: true,
      }),
    ];
  },
});
