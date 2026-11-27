/**
 * What kind of diagram a source is, in words a reader is told: the first word
 * of the source after its settings and comments, which is how the diagram
 * library itself tells the kinds apart.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

const KINDS: Record<string, () => string> = {
  flowchart: m.editor_diagram_kind_flowchart,
  graph: m.editor_diagram_kind_flowchart,
  sequencediagram: m.editor_diagram_kind_sequence,
  classdiagram: m.editor_diagram_kind_class,
  statediagram: m.editor_diagram_kind_state,
  erdiagram: m.editor_diagram_kind_er,
  gantt: m.editor_diagram_kind_gantt,
  pie: m.editor_diagram_kind_pie,
  mindmap: m.editor_diagram_kind_mindmap,
  timeline: m.editor_diagram_kind_timeline,
  journey: m.editor_diagram_kind_journey,
  gitgraph: m.editor_diagram_kind_git,
};

/** The word that names the kind, lowercased, without a version (`stateDiagram-v2` is `statediagram`). */
export function diagramKeyword(source: string): string | undefined {
  let rest = source.replace(/^\uFEFF/, '');
  // Front matter: settings between two lines of three dashes.
  const front = /^\s*---\r?\n[\s\S]*?\r?\n---[^\n]*(?:\n|$)/.exec(rest);
  if (front) rest = rest.slice(front[0].length);
  for (const line of rest.split('\n')) {
    const trimmed = line.trim();
    // Blank lines, comments and directives (`%%{ … }%%`) all start with nothing or `%%`.
    if (trimmed === '' || trimmed.startsWith('%%')) continue;
    const word = /^[A-Za-z][\w-]*/.exec(trimmed)?.[0];
    return word?.toLowerCase().replace(/-v\d+$/, '');
  }
  return undefined;
}

/** The kind of diagram in the reader's language; "Diagram" when it is not one of the known kinds. */
export function diagramKind(source: string): string {
  const keyword = diagramKeyword(source);
  const name = keyword !== undefined && Object.hasOwn(KINDS, keyword) ? KINDS[keyword] : undefined;
  return (name ?? m.editor_diagram_label)();
}
