import type { Editor } from "@tiptap/core";
import { Fragment, type Node as PmNode, type Schema } from "@tiptap/pm/model";
import type { EditorState, Transaction } from "@tiptap/pm/state";
import { yUndoPluginKey } from "@tiptap/y-tiptap";
import type * as Y from "yjs";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { yDocToTiptapJson } from "./collab-tiptap.js";
import { FVOCI_YDOC_FRAGMENT } from "./collab/constants.js";
import { UNIQUE_ID_NODE_TYPES } from "./extract.js";
import type { TiptapDoc } from "./json.js";
import { mdToTiptapJson } from "./markdown/parse.js";
import { tiptapDocToMd } from "./md.js";
import { sameValue } from "./vue/use-editor-state.js";

type SourceNode = {
  type: string;
  attrs?: Record<string, unknown>;
  text?: string;
  marks?: { type: string; attrs?: Record<string, unknown> }[];
  content?: SourceNode[];
};
export type ModeScope = string | number | undefined;
export type SourceDiagnostic = { path: string; id?: string; field: string; reason: string };
type Span = { start: number; end: number; node: SourceNode };
export type SourceCapture = {
  readonly source: string;
  readonly epoch: number;
  readonly scope: ModeScope;
  readonly doc: PmNode;
  readonly spans: readonly Span[];
};
export type SourceProposal = {
  readonly capture: SourceCapture;
  readonly source: string;
  readonly status: "ready" | "noop" | "loss" | "stale" | "invalid";
  readonly diagnostics: readonly SourceDiagnostic[];
  readonly total: number;
  readonly transaction?: Transaction;
};

/** y-tiptap deletes elements it cannot construct. Inspect stored node/mark
 * names before any SDK binding, so an unsupported stored document remains
 * read-only rather than being repaired as a side effect of opening it. */
export function rawEditorPreflight(ydoc: Y.Doc, schema: Schema): SourceDiagnostic[] {
  const raw = yDocToTiptapJson(ydoc);
  const result: SourceDiagnostic[] = [];
  function visit(node: SourceNode, path: string): void {
    if (!schema.nodes[node.type])
      result.push(
        diagnostic(
          node,
          path,
          "type",
          `Unsupported stored node ${node.type}; raw data is preserved.`,
        ),
      );
    for (const mark of node.marks ?? [])
      if (!schema.marks[mark.type])
        result.push(
          diagnostic(
            node,
            path,
            `marks.${mark.type}`,
            "Unsupported stored mark; raw data is preserved.",
          ),
        );
    node.content?.forEach((child, index) => {
      visit(child, `${path}.content.${String(index)}`);
    });
  }
  children(raw).forEach((node, index) => {
    visit(node, `content.${String(index)}`);
  });
  if (!result.length && (raw.content?.length ?? 0) > 0) {
    try {
      schema.nodeFromJSON(raw).check();
    } catch (error) {
      result.push({
        path: "document",
        field: "content/schema",
        reason: error instanceof Error ? error.message : "Unsupported stored content",
      });
    }
  }
  return result;
}

const blockTypes: ReadonlySet<string> = new Set(UNIQUE_ID_NODE_TYPES);
const syntaxParser = unified().use(remarkParse).use(remarkGfm).use(remarkMath);

function unsupportedSyntax(source: string): SourceDiagnostic[] {
  const result: SourceDiagnostic[] = [];
  function visit(value: unknown): void {
    if (typeof value !== "object" || value === null || !("type" in value)) return;
    const node = value as {
      type: string;
      value?: string;
      children?: unknown[];
      position?: { start: { offset?: number } };
    };
    if (
      ["footnoteDefinition", "footnoteReference", "linkReference", "imageReference"].includes(
        node.type,
      ) ||
      (node.type === "html" &&
        !/^(?:<\/?u>|<br\s*\/?>|<details>\s*<summary>[\s\S]*?<\/summary>\s*|<\/details>\s*)$/i.test(
          node.value ?? "",
        ))
    ) {
      result.push({
        path: `source.${String(node.position?.start.offset ?? 0)}`,
        field: node.type,
        reason:
          "The existing Markdown parser cannot preserve this construct; use rich editing or Cancel.",
      });
    }
    node.children?.forEach(visit);
  }
  visit(syntaxParser.parse(source));
  return result;
}

function readNode(value: unknown): SourceNode {
  if (
    typeof value !== "object" ||
    value === null ||
    !("type" in value) ||
    typeof value.type !== "string"
  )
    throw new Error("Invalid stored node");
  // Schema-free observation is deliberate. Never parse raw stored data through
  // the PM schema to decide that an unknown field was not there.
  return value as SourceNode;
}

function children(doc: TiptapDoc): SourceNode[] {
  return (doc.content ?? []).map(readNode);
}

function markdown(node: SourceNode): string {
  function references(raw: SourceNode): SourceNode {
    if (raw.type === "mention") {
      const entity = raw.attrs?.entity;
      const id = raw.attrs?.id;
      const label = raw.attrs?.label;
      if (
        typeof id === "string" &&
        typeof label === "string" &&
        typeof entity === "string" &&
        !/[\]|\n]/.test(id + label)
      ) {
        const token =
          entity === "user" || entity === "group"
            ? `@[${entity}:${id}|${label}]`
            : ["document", "task", "project"].includes(entity)
              ? `[[${entity === "document" ? "doc" : entity}:${id}|${label}]]`
              : null;
        if (token) return { type: "text", text: token, ...(raw.marks ? { marks: raw.marks } : {}) };
      }
    }
    return { ...raw, ...(raw.content ? { content: raw.content.map(references) } : {}) };
  }
  return tiptapDocToMd({ type: "doc", content: [references(node)] }).replace(/\n$/, "");
}

function diagnostic(
  node: SourceNode,
  path: string,
  field: string,
  reason: string,
): SourceDiagnostic {
  const id = node.attrs?.id;
  return { path, ...(typeof id === "string" ? { id } : {}), field, reason };
}

function identityDiagnostics(nodes: readonly SourceNode[]): SourceDiagnostic[] {
  const ids = new Map<string, string>();
  const result: SourceDiagnostic[] = [];
  function visit(node: SourceNode, path: string): void {
    if (blockTypes.has(node.type)) {
      const id = node.attrs?.id;
      if (typeof id !== "string" || id.length === 0)
        result.push(
          diagnostic(node, path, "id", "Missing block identity; return to rich editing."),
        );
      else if (ids.has(id))
        result.push(
          diagnostic(node, path, "id", `Duplicate block identity at ${ids.get(id) ?? ""}.`),
        );
      else ids.set(id, path);
    }
    node.content?.forEach((child, i) => {
      visit(child, `${path}.content.${String(i)}`);
    });
  }
  nodes.forEach((node, i) => {
    visit(node, `content.${String(i)}`);
  });
  return result;
}

class Loss extends Error {
  constructor(readonly detail: SourceDiagnostic) {
    super(detail.reason);
  }
}

function loss(node: SourceNode, path: string, field: string, reason: string): never {
  throw new Loss(diagnostic(node, path, field, reason));
}

/** A three-way merge of raw data, its parsed projection and the edited source.
 * Only fields actually changed by the user replace stored values. Hidden attrs
 * and marks retain their original values. Ambiguous shapes never get guessed. */
function mergeNode(raw: SourceNode, base: SourceNode, next: SourceNode, path: string): SourceNode {
  if (sameValue(base, next)) return raw;
  if (raw.type !== base.type)
    loss(
      raw,
      path,
      "type",
      `Markdown represents ${raw.type} as ${base.type}; edit this range in rich mode.`,
    );
  if (base.type !== next.type) {
    if (raw.type === "attachment" || raw.type === "embed" || raw.type === "mention")
      loss(
        raw,
        path,
        "type/reference",
        "This change would remove an existing reference or file relationship.",
      );
    // Node conversion can retain the identity of a single, mapped block; child
    // identities cannot be reassigned across a different container shape.
    if (raw.content?.some((node) => !["text", "hardBreak"].includes(node.type)))
      loss(raw, path, "content/type", "Container conversion would discard child identities.");
    if (["paragraph", "heading"].includes(raw.type) && ["paragraph", "heading"].includes(next.type))
      return mergeNode({ ...raw, type: next.type }, { ...base, type: next.type }, next, path);
    if (!sameValue(raw.content, base.content))
      loss(
        raw,
        path,
        "content/marks",
        "Node conversion would lose formatting or literal data omitted from Markdown.",
      );
    for (const [field, value] of Object.entries(raw.attrs ?? {})) {
      if (
        field !== "id" &&
        value !== null &&
        value !== undefined &&
        !sameValue(value, base.attrs?.[field])
      )
        loss(
          raw,
          path,
          `attrs.${field}`,
          "Node conversion would lose a stored attribute that Markdown does not represent.",
        );
    }
    return { ...next, attrs: { ...next.attrs, ...(raw.attrs?.id ? { id: raw.attrs.id } : {}) } };
  }
  const attrs = { ...raw.attrs };
  for (const field of new Set([
    ...Object.keys(base.attrs ?? {}),
    ...Object.keys(next.attrs ?? {}),
  ])) {
    const before = base.attrs?.[field];
    const after = next.attrs?.[field];
    if (sameValue(before, after)) continue;
    if (field === "id" || field === "ref" || field === "entity")
      loss(
        raw,
        path,
        field,
        "Existing block, entity and attachment targets cannot be retargeted by a lossy projection.",
      );
    if (Object.hasOwn(next.attrs ?? {}, field)) attrs[field] = after;
    else Reflect.deleteProperty(attrs, field);
  }
  let content: SourceNode[] | undefined;
  if (base.content || next.content) {
    const original = raw.content ?? [];
    const projected = base.content ?? [];
    if (raw.type === "paragraph" || raw.type === "heading" || raw.type === "detailsSummary") {
      content = mergeInline(original, projected, next.content ?? [], path);
    } else if (original.length !== projected.length)
      loss(
        raw,
        path,
        "content",
        "Markdown flattened or omitted content in this range; return to rich editing.",
      );
    else content = mergeChildren(original, projected, next.content ?? [], path);
  }
  let marks = raw.marks;
  if (!sameValue(base.marks, next.marks)) {
    const kept = new Map((raw.marks ?? []).map((mark) => [mark.type, mark]));
    for (const mark of base.marks ?? []) {
      const after = next.marks?.find((item) => item.type === mark.type);
      if (!after) kept.delete(mark.type);
      else if (!sameValue(mark, after)) {
        const original = kept.get(mark.type);
        kept.set(mark.type, { ...after, attrs: { ...original?.attrs, ...after.attrs } });
      }
    }
    for (const mark of next.marks ?? [])
      if (!base.marks?.some((item) => item.type === mark.type)) kept.set(mark.type, mark);
    marks = [...kept.values()];
  }
  if (raw.text !== base.text && next.text !== base.text)
    loss(
      raw,
      path,
      "text",
      "Markdown changed whitespace, Unicode or literal/reference meaning in this range.",
    );
  return {
    ...raw,
    ...(Object.keys(attrs).length ? { attrs } : {}),
    ...(next.text !== undefined ? { text: next.text } : {}),
    ...(content !== undefined ? { content } : {}),
    ...(marks !== undefined ? { marks } : {}),
  };
}

function mergeInline(
  raw: SourceNode[],
  base: SourceNode[],
  next: SourceNode[],
  path: string,
): SourceNode[] {
  // Atom order and identity are never inferred from equal labels. For the
  // ordinary text case, use UTF-16 semantic offsets to retain hidden marks on
  // unaffected text and on a uniquely mapped edited run, even when the parser
  // splits that run to introduce an emphasis/link boundary.
  if ([...raw, ...base, ...next].some((node) => node.type !== "text")) {
    if (raw.length !== base.length)
      loss(
        raw[0] ?? { type: "paragraph" },
        path,
        "inline atoms",
        "Projection changed inline atom boundaries or identity.",
      );
    return mergeChildren(raw, base, next, path);
  }
  const text = (nodes: SourceNode[]) => nodes.map((node) => node.text ?? "").join("");
  const before = text(base);
  const after = text(next);
  if (text(raw) !== before)
    loss(
      raw[0] ?? { type: "paragraph" },
      path,
      "text",
      "Projection changed literal text or Unicode before editing.",
    );
  let prefix = 0;
  while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix])
    prefix++;
  let suffix = 0;
  while (
    suffix < before.length - prefix &&
    suffix < after.length - prefix &&
    before[before.length - 1 - suffix] === after[after.length - 1 - suffix]
  )
    suffix++;
  const delta = after.length - before.length;
  const spans = (nodes: SourceNode[]) => {
    let offset = 0;
    return nodes.map((node) => {
      const start = offset;
      offset += node.text?.length ?? 0;
      return { node, start, end: offset };
    });
  };
  const rawSpans = spans(raw);
  const baseSpans = spans(base);
  const output: SourceNode[] = [];
  for (const run of spans(next)) {
    let offset = run.start;
    while (offset < run.end) {
      const originalOffset =
        offset < prefix ? offset : offset >= after.length - suffix ? offset - delta : prefix;
      const original =
        rawSpans.find((span) => span.start <= originalOffset && span.end > originalOffset) ??
        rawSpans.at(-1);
      const projected =
        baseSpans.find((span) => span.start <= originalOffset && span.end > originalOffset) ??
        baseSpans.at(-1);
      if (!original || !projected) {
        output.push({ ...run.node, text: after.slice(offset, run.end) });
        break;
      }
      const end =
        offset < prefix
          ? Math.min(run.end, prefix, original.end, projected.end)
          : offset >= after.length - suffix
            ? Math.min(run.end, original.end + delta, projected.end + delta)
            : Math.min(run.end, after.length - suffix);
      const merged = mergeNode(
        { ...original.node, text: "mapped" },
        { ...projected.node, text: "mapped" },
        { ...run.node, text: "mapped" },
        path,
      );
      output.push({ ...merged, text: after.slice(offset, end) });
      offset = end;
    }
  }
  return output;
}

function mergeChildren(
  raw: SourceNode[],
  base: SourceNode[],
  next: SourceNode[],
  path: string,
): SourceNode[] {
  let prefix = 0;
  while (prefix < base.length && prefix < next.length && sameValue(base[prefix], next[prefix]))
    prefix++;
  let suffix = 0;
  while (
    suffix < base.length - prefix &&
    suffix < next.length - prefix &&
    sameValue(base[base.length - 1 - suffix], next[next.length - 1 - suffix])
  )
    suffix++;
  const oldMiddle = base.slice(prefix, base.length - suffix);
  const newMiddle = next.slice(prefix, next.length - suffix);
  const middle = newMiddle.map((node, i) => {
    const exact = base
      .map((candidate, index) => (sameValue(candidate, node) ? index : -1))
      .filter((index) => index >= 0);
    if (exact.length > 1)
      loss(node, path, "identity", "Repeated source content has an ambiguous identity mapping.");
    if (exact.length === 1) return raw[exact[0] ?? -1] ?? node;
    const original = raw[prefix + i];
    const projected = oldMiddle[i];
    if (oldMiddle.length === newMiddle.length && original && projected)
      return mergeNode(original, projected, node, `${path}.content.${String(prefix + i)}`);
    if (oldMiddle.length === 1 && newMiddle.length > 1) {
      const firstRaw = raw[prefix];
      const firstBase = oldMiddle[0];
      if (i === 0 && firstRaw && firstBase)
        return mergeNode(firstRaw, firstBase, node, `${path}.content.${String(prefix)}`);
      return node;
    }
    if (
      newMiddle.length === 1 &&
      oldMiddle.length > 1 &&
      original &&
      projected &&
      oldMiddle.every(
        (item) =>
          item.type === "paragraph" && (item.content ?? []).every((child) => child.type === "text"),
      ) &&
      raw
        .slice(prefix, base.length - suffix)
        .every((item) => sameValue(item.content?.[0]?.marks, original.content?.[0]?.marks))
    )
      return mergeNode(original, projected, node, `${path}.content.${String(prefix)}`);
    // A pure insertion has no existing identity to guess; UniqueID allocates
    // only when its actual localized apply transaction reaches the editor.
    if (oldMiddle.length === 0) return node;
    loss(
      original ?? node,
      path,
      "content/identity",
      "Splitting or joining this range has ambiguous existing block identities.",
    );
  });
  return [...raw.slice(0, prefix), ...middle, ...raw.slice(raw.length - suffix)];
}

/** Descend matching containers; text edits use the smallest PM slice. Keeping
 * the existing Y element and its unchanged text structs makes later peer text
 * survive this actor's undo. No full document/fragment replacement is used. */
function patchNode(tr: Transaction, old: PmNode, next: PmNode, pos: number): void {
  if (old.eq(next)) return;
  if (old.isText && next.isText && old.sameMarkup(next)) {
    const before = Fragment.from(old);
    const after = Fragment.from(next);
    const start = before.findDiffStart(after);
    const end = before.findDiffEnd(after);
    if (start === null || !end) return;
    const overlap = Math.max(0, start - Math.min(end.a, end.b));
    tr.replaceWith(
      tr.mapping.map(pos + start),
      tr.mapping.map(pos + end.a + overlap),
      after.cut(start, end.b + overlap),
    );
    return;
  }
  if (old.type === next.type && old.childCount === next.childCount && !old.isText) {
    if (!old.sameMarkup(next))
      tr.setNodeMarkup(tr.mapping.map(pos), next.type, next.attrs, next.marks);
    let offset = 1;
    for (let i = 0; i < old.childCount; i++) {
      const child = old.child(i);
      patchNode(tr, child, next.child(i), pos + offset);
      offset += child.nodeSize;
    }
    return;
  }
  const start = old.content.findDiffStart(next.content);
  const end = old.content.findDiffEnd(next.content);
  if (!old.isLeaf && old.sameMarkup(next) && start !== null && end) {
    const overlap = start - Math.min(end.a, end.b);
    const oldEnd = end.a + Math.max(0, overlap);
    const newEnd = end.b + Math.max(0, overlap);
    tr.replaceWith(
      tr.mapping.map(pos + 1 + start),
      tr.mapping.map(pos + 1 + oldEnd),
      next.content.cut(start, newEnd),
    );
  } else tr.replaceWith(tr.mapping.map(pos), tr.mapping.map(pos + old.nodeSize), next);
}

export function sourceTransaction(
  state: EditorState,
  from: number,
  oldCount: number,
  next: readonly PmNode[],
): Transaction {
  const tr = state.tr;
  const old: PmNode[] = [];
  let pos = from;
  for (let i = 0; i < oldCount; i++) {
    const node = state.doc.nodeAt(pos);
    if (!node) throw new Error("Source range no longer exists");
    old.push(node);
    pos += node.nodeSize;
  }
  if (oldCount === next.length) {
    pos = from;
    for (let i = 0; i < old.length; i++) {
      const before = old[i];
      const after = next[i];
      if (!before || !after) throw new Error("Invalid source range");
      patchNode(tr, before, after, pos);
      pos += before.nodeSize;
    }
  } else tr.replaceWith(from, pos, Fragment.fromArray([...next]));
  tr.doc.check();
  return tr.setStoredMarks(state.storedMarks);
}

export class SourceModeSession {
  private epoch = 0;
  private retired = false;
  private readonly onContent = () => {
    this.epoch++;
  };
  private readonly fragment: Y.XmlFragment;
  constructor(
    readonly ydoc: Y.Doc,
    private readonly scope: () => ModeScope,
    private readonly authorized: () => boolean,
  ) {
    this.fragment = ydoc.getXmlFragment(FVOCI_YDOC_FRAGMENT);
    this.fragment.observeDeep(this.onContent);
  }
  capture(doc: PmNode): SourceCapture {
    const spans: Span[] = [];
    let source = "";
    for (const node of children(yDocToTiptapJson(this.ydoc))) {
      if (spans.length) source += "\n\n";
      const start = source.length;
      source += markdown(node);
      spans.push({ start, end: source.length, node });
    }
    return { source, epoch: this.epoch, scope: this.scope(), doc, spans };
  }
  isCurrent(capture: SourceCapture): boolean {
    return !this.retired && capture.epoch === this.epoch && Object.is(capture.scope, this.scope());
  }
  prepare(capture: SourceCapture, source: string, state: EditorState): SourceProposal {
    const result = (
      status: SourceProposal["status"],
      diagnostics: SourceDiagnostic[] = [],
      transaction?: Transaction,
    ): SourceProposal => ({
      capture,
      source,
      status,
      diagnostics: diagnostics.slice(0, 20),
      total: diagnostics.length,
      ...(transaction ? { transaction } : {}),
    });
    if (!this.isCurrent(capture) || !state.doc.eq(capture.doc)) return result("stale");
    if (source === capture.source) return result("noop");
    // Detect dropped syntax using the same installed Markdown grammar before
    // the existing converter discards constructs it does not implement.
    const syntax = unsupportedSyntax(source);
    if (syntax.length) return result("loss", syntax);
    if (
      capture.spans.length !== state.doc.childCount ||
      capture.spans.some((span, i) => span.node.type !== state.doc.child(i).type.name)
    )
      return result("loss", [
        {
          path: "document",
          field: "raw/schema",
          reason:
            "The live editor cannot represent the stored node layout; preserve it and inspect in read-only mode.",
        },
      ]);
    const identity = identityDiagnostics(capture.spans.map((span) => span.node));
    if (identity.length) return result("loss", identity);
    try {
      let start = 0;
      while (
        start < source.length &&
        start < capture.source.length &&
        source[start] === capture.source[start]
      )
        start++;
      let suffix = 0;
      while (
        suffix < source.length - start &&
        suffix < capture.source.length - start &&
        source[source.length - 1 - suffix] === capture.source[capture.source.length - 1 - suffix]
      )
        suffix++;
      const end = capture.source.length - suffix;
      let first = capture.spans.findIndex((span) => span.end >= start);
      if (first < 0) first = capture.spans.length - 1;
      let last = first;
      while (last + 1 < capture.spans.length && (capture.spans[last + 1]?.start ?? Infinity) <= end)
        last++;
      const beforeSpan = capture.spans[first];
      const afterSpan = capture.spans[last];
      if (!beforeSpan || !afterSpan) return result("invalid");
      const regionStart = beforeSpan.start;
      const regionEnd = afterSpan.end;
      const delta = source.length - capture.source.length;
      const raw = capture.spans.slice(first, last + 1).map((span) => span.node);
      const base = children(mdToTiptapJson(capture.source.slice(regionStart, regionEnd)));
      const parsed = children(mdToTiptapJson(source.slice(regionStart, regionEnd + delta)));
      if (raw.length !== base.length)
        loss(
          raw[0] ?? { type: "doc" },
          `content.${String(first)}`,
          "content",
          "Markdown omitted or combined blocks in this range.",
        );
      const merged = mergeChildren(raw, base, parsed, "doc");
      const identities = identityDiagnostics([
        ...capture.spans.slice(0, first).map((span) => span.node),
        // New nodes have no ID until the editor's existing UniqueID extension
        // processes the actual write, so only existing mapped nodes are checked.
        ...merged.filter((node) => raw.includes(node) || typeof node.attrs?.id === "string"),
        ...capture.spans.slice(last + 1).map((span) => span.node),
      ]);
      if (identities.length) return result("loss", identities);
      let from = 0;
      for (let i = 0; i < first; i++) from += state.doc.child(i).nodeSize;
      const nodes = merged.map((node, index) =>
        checkedNode(state.schema, node, `content.${String(first + index)}`),
      );
      const transaction = sourceTransaction(state, from, raw.length, nodes);
      return result(transaction.docChanged ? "ready" : "noop", [], transaction);
    } catch (error) {
      return result(
        error instanceof Loss ? "loss" : "invalid",
        error instanceof Loss
          ? [error.detail]
          : [
              {
                path: "document",
                field: "syntax/schema",
                reason: error instanceof Error ? error.message : "Invalid Markdown",
              },
            ],
      );
    }
  }
  apply(proposal: SourceProposal, editor: Editor): boolean {
    if (
      proposal.status !== "ready" ||
      !proposal.transaction ||
      !this.authorized() ||
      !this.isCurrent(proposal.capture) ||
      editor.isDestroyed ||
      editor.view.composing ||
      !editor.state.doc.eq(proposal.capture.doc)
    )
      return false;
    const undo = yUndoPluginKey.getState(editor.state) as
      { undoManager?: Y.UndoManager } | undefined;
    undo?.undoManager?.stopCapturing();
    editor.view.dispatch(proposal.transaction.setMeta("fvociSourceMode", true));
    undo?.undoManager?.stopCapturing();
    return true;
  }
  destroy(): void {
    if (this.retired) return;
    this.retired = true;
    this.fragment.unobserveDeep(this.onContent);
  }
}

function checkedNode(schema: Schema, node: SourceNode, path: string): PmNode {
  function inspect(raw: SourceNode, path: string): void {
    const type = schema.nodes[raw.type];
    if (!type) loss(raw, path, "type", "Unknown stored node cannot be changed through Markdown.");
    for (const key of Object.keys(raw.attrs ?? {})) {
      if (!Object.hasOwn(type.spec.attrs ?? {}, key))
        loss(
          raw,
          path,
          `attrs.${key}`,
          "Stored extension attribute is outside the editor schema; this range stays read-only.",
        );
    }
    for (const mark of raw.marks ?? []) {
      const type = schema.marks[mark.type];
      if (!type)
        loss(
          raw,
          path,
          `marks.${mark.type}`,
          "Stored extension mark is outside the editor schema.",
        );
      for (const key of Object.keys(mark.attrs ?? {}))
        if (!Object.hasOwn(type.spec.attrs ?? {}, key))
          loss(
            raw,
            path,
            `marks.${mark.type}.${key}`,
            "Stored extension mark attribute is outside the editor schema.",
          );
    }
    raw.content?.forEach((child, index) => {
      inspect(child, `${path}.content.${String(index)}`);
    });
  }
  inspect(node, path);
  const result = schema.nodeFromJSON(node);
  result.check();
  return result;
}
