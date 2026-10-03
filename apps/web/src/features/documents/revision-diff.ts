/** Observational comparison of two immutable, authorized revision projections.
 * Block-ID types come from the existing editor schema contract at the caller.
 * Attachment/mention IDs are resource references, never block identities. */
export type RevisionProjection = {
  id: string;
  targetKind: string;
  targetId: string;
  contentJson: unknown;
};
export type RevisionChangeKind =
  | "added"
  | "removed"
  | "moved"
  | "text"
  | "checkbox"
  | "table"
  | "link"
  | "attachment"
  | "reference"
  | "format"
  | "attributes"
  | "structure";
export type RevisionBlockView = {
  path: readonly number[];
  type: string;
  blockId: string | null;
  text: string;
  /** Safe data for a textual before/after preview; never rendered as HTML. */
  content: unknown;
};
export type RevisionChange = {
  key: string;
  kind: RevisionChangeKind;
  identity: "block-id" | "position" | "unmatched";
  before: RevisionBlockView | null;
  after: RevisionBlockView | null;
  /** Changed field/run summaries preserve literal values, including nontext. */
  values?: { before: unknown; after: unknown };
};
export type RevisionDiff = {
  beforeId: string;
  afterId: string;
  changes: RevisionChange[];
  limits: (
    "missing-ids" | "duplicate-ids" | "positional-match" | "invalid-content" | "size-limit"
  )[];
};
type Node = Record<string, unknown> & { type: string };
type Block = RevisionBlockView & { node: Node; parent: Block | null; order: number };
const MAX_NODES = 20000;
const MAX_DEPTH = 64;
const MAX_TEXT = 2 * 1024 * 1024;
function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}
function node(value: unknown): Node | null {
  const valueRecord = record(value);
  return valueRecord && typeof valueRecord.type === "string" ? (valueRecord as Node) : null;
}
function children(value: Node): unknown[] {
  return Array.isArray(value.content) ? value.content : [];
}
/** Object key order and mark-set order carry no document meaning. Child/run
 * order, null, unknown attributes and resource references do carry meaning. */
function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  const object = record(value);
  if (!object) return value;
  return Object.fromEntries(
    Object.keys(object)
      .sort()
      .map((key) => {
        const field = object[key];
        let normalized =
          key === "marks" && Array.isArray(field)
            ? field
                .map(canonical)
                .sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)))
            : canonical(field);
        if (key === "content" && Array.isArray(normalized)) {
          const merged: unknown[] = [];
          for (const child of normalized) {
            const current = node(child),
              prior = node(merged.at(-1));
            if (
              current?.type === "text" &&
              prior?.type === "text" &&
              typeof current.text === "string" &&
              typeof prior.text === "string" &&
              same({ ...current, text: "" }, { ...prior, text: "" })
            )
              prior.text += current.text;
            else merged.push(child);
          }
          normalized = merged;
        }
        return [key, normalized];
      }),
  );
}
function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b))
    return a.length === b.length && a.every((value, index) => same(value, b[index]));
  const left = record(a),
    right = record(b);
  if (!left || !right) return false;
  const keys = Object.keys(left);
  return (
    keys.length === Object.keys(right).length &&
    keys.every((key) => Object.hasOwn(right, key) && same(left[key], right[key]))
  );
}
function literalText(value: unknown): string {
  return typeof value === "string" ? value : "";
}
function textOf(n: Node): string {
  if (typeof n.text === "string") return n.text;
  const attrs = record(n.attrs);
  if (n.type === "hardBreak") return "\n";
  if (n.type === "mention") return literalText(attrs?.label ?? attrs?.id);
  if (n.type === "attachment") return literalText(attrs?.name);
  if (n.type === "embed") return literalText(attrs?.ref);
  if (n.type === "math" || n.type === "mathInline") return literalText(attrs?.latex);
  if (n.type === "mermaid") return literalText(attrs?.source);
  const separator = [
    "doc",
    "table",
    "tableRow",
    "tableCell",
    "tableHeader",
    "taskList",
    "taskItem",
    "bulletList",
    "orderedList",
    "listItem",
    "blockquote",
  ].includes(n.type)
    ? "\n"
    : "";
  return children(n)
    .map((child) => {
      const parsed = node(child);
      return parsed ? textOf(parsed) : "";
    })
    .join(separator);
}
function validateBounds(root: unknown): void {
  const ancestors = new Set<object>();
  let count = 0,
    strings = 0;
  function walk(value: unknown, depth: number): void {
    if (++count > MAX_NODES * 12 || depth > MAX_DEPTH * 3) throw new Error("size-limit");
    if (typeof value === "string") {
      strings += value.length;
      if (strings > MAX_TEXT) throw new Error("size-limit");
    }
    if (!value || typeof value !== "object") return;
    if (ancestors.has(value)) throw new Error("invalid-content");
    ancestors.add(value);
    for (const field of Object.values(value)) walk(field, depth + 1);
    ancestors.delete(value);
  }
  walk(root, 0);
}
function collect(
  root: unknown,
  types: ReadonlySet<string>,
  limits: Set<RevisionDiff["limits"][number]>,
): Block[] {
  const doc = node(root);
  if (!doc || doc.type !== "doc" || (doc.content !== undefined && !Array.isArray(doc.content))) {
    limits.add("invalid-content");
    return [];
  }
  const blocks: Block[] = [];
  let visited = 0,
    textSize = 0;
  function walk(value: unknown, path: number[], parent: Block | null, depth: number): void {
    if (++visited > MAX_NODES || depth > MAX_DEPTH) throw new Error("size-limit");
    const n = node(value);
    if (!n || (n.content !== undefined && !Array.isArray(n.content))) {
      limits.add("invalid-content");
      return;
    }
    if (typeof n.text === "string") textSize += n.text.length;
    if (textSize > MAX_TEXT) throw new Error("size-limit");
    let owner = parent;
    // Top-level unknown nodes are observable too, without claiming identity.
    if (types.has(n.type) || n.type === "attachment" || path.length === 1) {
      const id = record(n.attrs)?.id;
      const blockId = types.has(n.type) && typeof id === "string" && id.length > 0 ? id : null;
      if (types.has(n.type) && !blockId) limits.add("missing-ids");
      owner = {
        node: n,
        path,
        parent,
        order: blocks.length,
        type: n.type,
        blockId,
        text: "",
        content: n,
      };
      blocks.push(owner);
    }
    children(n).forEach((child, index) => {
      walk(child, [...path, index], owner, depth + 1);
    });
  }
  children(doc).forEach((child, index) => {
    walk(child, [index], null, 1);
  });
  // Validate bounds before any recursive derived display/normalization work.
  if (!limits.has("invalid-content")) for (const block of blocks) block.text = textOf(block.node);
  return blocks;
}
function view(block: Block | null): RevisionBlockView | null {
  return (
    block && {
      path: block.path,
      type: block.type,
      blockId: block.blockId,
      text: block.text,
      content: block.content,
    }
  );
}
/** Stable relative order is an increasing subsequence of the common siblings;
 * new/deleted blocks do not turn simple index shifts into moves. */
function stableOrder(pairs: readonly [Block, Block][]): Set<Block> {
  const ordered = [...pairs].sort((a, b) => a[1].order - b[1].order);
  const tails: number[] = [],
    previous: number[] = [];
  function orderedEntry(index: number): [Block, Block] {
    const entry = ordered[index];
    if (!entry) throw new Error("Invalid comparison order");
    return entry;
  }
  ordered.forEach(([before], index) => {
    let low = 0,
      high = tails.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (orderedEntry(tails[middle] ?? -1)[0].order < before.order) low = middle + 1;
      else high = middle;
    }
    previous[index] = low ? (tails[low - 1] ?? -1) : -1;
    tails[low] = index;
  });
  const stable = new Set<Block>();
  let index = tails.at(-1) ?? -1;
  while (index >= 0) {
    stable.add(orderedEntry(index)[1]);
    index = previous[index] ?? -1;
  }
  return stable;
}
function localProjection(block: Block, types: ReadonlySet<string>): unknown {
  function project(n: Node, top: boolean): unknown {
    if (!top && (types.has(n.type) || n.type === "attachment")) return { block: n.type };
    return {
      ...n,
      ...(n.content === undefined
        ? {}
        : {
            content: children(n).map((child) => {
              const parsed = node(child);
              return parsed ? project(parsed, false) : child;
            }),
          }),
    };
  }
  return canonical(project(block.node, true));
}
function fields(block: Block, types: ReadonlySet<string>): Record<RevisionChangeKind, unknown[]> {
  const out: Record<RevisionChangeKind, unknown[]> = {
    added: [],
    removed: [],
    moved: [],
    text: [],
    checkbox: [],
    table: [],
    link: [],
    attachment: [],
    reference: [],
    format: [],
    attributes: [],
    structure: [],
  };
  function walk(n: Node, path: number[], top: boolean): void {
    if (!top && (types.has(n.type) || n.type === "attachment")) return;
    const attrs = record(n.attrs) ?? {};
    if (typeof n.text === "string") out.text.push(n.text);
    if (n.type === "taskItem") out.checkbox.push(attrs.checked);
    if (n.type === "attachment") out.attachment.push(canonical(attrs));
    if (n.type === "mention" || n.type === "embed")
      out.reference.push({ path, type: n.type, attrs: canonical(attrs) });
    const marks = Array.isArray(n.marks) ? n.marks : [];
    const otherMarks: unknown[] = [];
    for (const mark of marks) {
      const m = node(mark);
      if (m?.type === "link")
        out.link.push({ path, text: n.text ?? null, attrs: canonical(m.attrs) });
      else otherMarks.push(canonical(mark));
    }
    if (otherMarks.length)
      out.format.push({
        path,
        text: n.text ?? null,
        marks: otherMarks.sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b))),
      });
    if (n.type !== "attachment" && n.type !== "mention" && n.type !== "embed") {
      const rest = Object.fromEntries(
        Object.entries(attrs).filter(
          ([key]) =>
            !(top && types.has(n.type) && key === "id") &&
            !(n.type === "taskItem" && key === "checked"),
        ),
      );
      if (Object.keys(rest).length) out.attributes.push({ path, attrs: canonical(rest) });
    }
    if (n.type.startsWith("table"))
      out.table.push({
        path,
        type: n.type,
        attrs: canonical(attrs),
        children: children(n).map((child) => node(child)?.type ?? null),
      });
    out.structure.push({
      path,
      type: n.type,
      children: children(n).map((child) => node(child)?.type ?? null),
    });
    children(n).forEach((child, index) => {
      const parsed = node(child);
      if (parsed) walk(parsed, [...path, index], false);
    });
  }
  walk(block.node, [], true);
  // Text nodes may be split into adjacent runs by a mark boundary.
  out.text = [out.text.join("")];
  return out;
}
export function compareRevisionProjections(
  before: RevisionProjection,
  after: RevisionProjection,
  blockIdTypes: readonly string[],
): RevisionDiff {
  if (before.targetKind !== after.targetKind || before.targetId !== after.targetId)
    throw new Error("Revision comparison requires the same target");
  const result: RevisionDiff = { beforeId: before.id, afterId: after.id, changes: [], limits: [] };
  const limits = new Set<RevisionDiff["limits"][number]>(),
    types = new Set(blockIdTypes);
  let left: Block[], right: Block[];
  try {
    validateBounds(before.contentJson);
    validateBounds(after.contentJson);
    left = collect(canonical(before.contentJson), types, limits);
    right = collect(canonical(after.contentJson), types, limits);
  } catch (error) {
    result.limits = [
      error instanceof Error && error.message === "invalid-content"
        ? "invalid-content"
        : "size-limit",
    ];
    return result;
  }
  if (limits.has("invalid-content")) {
    result.limits = [...limits];
    return result;
  }
  function byId(blocks: Block[]): Map<string, Block[]> {
    const map = new Map<string, Block[]>();
    for (const block of blocks)
      if (block.blockId) {
        const existing = map.get(block.blockId) ?? [];
        existing.push(block);
        map.set(block.blockId, existing);
        if (existing.length > 1) limits.add("duplicate-ids");
      }
    return map;
  }
  const leftIds = byId(left),
    rightIds = byId(right);
  const pairs = new Map<Block, { before: Block; identity: RevisionChange["identity"] }>();
  const used = new Set<Block>();
  for (const afterBlock of right) {
    const id = afterBlock.blockId;
    const beforeBlock =
      id && leftIds.get(id)?.length === 1 && rightIds.get(id)?.length === 1
        ? leftIds.get(id)?.[0]
        : undefined;
    if (beforeBlock && beforeBlock.type === afterBlock.type) {
      pairs.set(afterBlock, { before: beforeBlock, identity: "block-id" });
      used.add(beforeBlock);
    }
  }
  // Structural position is a comparison aid, never proof of historical node
  // identity. Only match unidentified same-type blocks under matched parents.
  const unidentifiedByPath = new Map(
    left.filter((block) => !block.blockId).map((block) => [block.path.join("."), block]),
  );
  for (const afterBlock of right) {
    if (pairs.has(afterBlock) || afterBlock.blockId) continue;
    const parent = afterBlock.parent ? pairs.get(afterBlock.parent)?.before : null;
    if (afterBlock.parent && !parent) continue;
    const beforeBlock = unidentifiedByPath.get(afterBlock.path.join("."));
    if (
      beforeBlock &&
      !used.has(beforeBlock) &&
      beforeBlock.type === afterBlock.type &&
      beforeBlock.parent === parent
    ) {
      pairs.set(afterBlock, { before: beforeBlock, identity: "position" });
      used.add(beforeBlock);
      if (!same(canonical(beforeBlock.node), canonical(afterBlock.node)))
        limits.add("positional-match");
    }
  }
  const siblingGroups = new Map<Block | null, [Block, Block][]>();
  for (const [afterBlock, pair] of pairs)
    if (pair.identity === "block-id") {
      const beforeParent = afterBlock.parent ? pairs.get(afterBlock.parent)?.before : null;
      if (pair.before.parent !== beforeParent) continue;
      const group = siblingGroups.get(afterBlock.parent) ?? [];
      group.push([pair.before, afterBlock]);
      siblingGroups.set(afterBlock.parent, group);
    }
  const stable = new Set<Block>();
  for (const group of siblingGroups.values())
    for (const block of stableOrder(group)) stable.add(block);
  function add(
    kind: RevisionChangeKind,
    beforeBlock: Block | null,
    afterBlock: Block | null,
    identity: RevisionChange["identity"],
    values?: RevisionChange["values"],
  ): void {
    result.changes.push({
      key: `${String(result.changes.length)}:${kind}`,
      kind,
      identity,
      before: view(beforeBlock),
      after: view(afterBlock),
      ...(values ? { values } : {}),
    });
  }
  for (const beforeBlock of left)
    if (!used.has(beforeBlock)) add("removed", beforeBlock, null, "unmatched");
  for (const afterBlock of right) {
    const pair = pairs.get(afterBlock);
    if (!pair) {
      add("added", null, afterBlock, "unmatched");
      continue;
    }
    const beforeBlock = pair.before;
    if (pair.identity === "block-id" && !stable.has(afterBlock))
      add("moved", beforeBlock, afterBlock, pair.identity);
    if (same(localProjection(beforeBlock, types), localProjection(afterBlock, types))) continue;
    const oldFields = fields(beforeBlock, types),
      newFields = fields(afterBlock, types);
    let reported = false;
    for (const kind of [
      "text",
      "checkbox",
      "table",
      "link",
      "attachment",
      "reference",
      "format",
      "attributes",
      "structure",
    ] as const) {
      if (!same(oldFields[kind], newFields[kind])) {
        add(kind, beforeBlock, afterBlock, pair.identity, {
          before: oldFields[kind],
          after: newFields[kind],
        });
        reported = true;
      }
    }
    // Preserve unsupported/unknown field changes rather than silently claiming
    // equal content from the named categories alone.
    if (!reported)
      add("attributes", beforeBlock, afterBlock, pair.identity, {
        before: localProjection(beforeBlock, types),
        after: localProjection(afterBlock, types),
      });
  }
  result.limits = [...limits];
  return result;
}
