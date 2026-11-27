/**
 * Localized labels, compact badges, and screen-reader wording for the four
 * dependency relation kinds (`fs`, `ss`, `ff`, `sf`) and lag days.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { RelationEdge } from '$lib/logic/dependencies.ts';
import type { RelationKind } from '$lib/logic/relations.ts';

/** Localized full name for a dependency relation kind. */
export function relationKindName(kind: RelationKind): string {
  switch (kind) {
    case 'fs':
      return m.gantt_relation_kind_fs();
    case 'ss':
      return m.gantt_relation_kind_ss();
    case 'ff':
      return m.gantt_relation_kind_ff();
    case 'sf':
      return m.gantt_relation_kind_sf();
  }
}

/** Localized day count for screen-reader sentences (`1 day` / `2 days`). */
export function relationDaySpan(absDays: number): string {
  return absDays === 1 ? m.gantt_relation_day_one() : m.gantt_relation_day_other({ days: absDays });
}

/**
 * Compact visual badge text for a relation link. Plain default links
 * (`fs` with `lagDays === 0`) return `''` so charts and chips stay uncluttered.
 */
export function relationBadge(kind: RelationKind, lagDays: number): string {
  if (kind === 'fs' && lagDays === 0) return '';
  const code = kind.toUpperCase();
  if (lagDays === 0) return code;
  const lag =
    lagDays > 0
      ? m.gantt_relation_lag_short_pos({ days: lagDays })
      : m.gantt_relation_lag_short_neg({ days: Math.abs(lagDays) });
  return `${code} ${lag}`;
}

/** Accessible name for a Gantt dependency arrow. */
export function arrowAccessibleLabel(
  fromTitle: string,
  toTitle: string,
  kind: RelationKind,
  lagDays: number,
  late: boolean,
  loop: boolean,
): string {
  const kindLabel = relationKindName(kind);
  const base =
    lagDays === 0
      ? m.gantt_arrow_label({ from: fromTitle, to: toTitle, kind: kindLabel })
      : lagDays > 0
        ? m.gantt_arrow_label_lag_pos({
            from: fromTitle,
            to: toTitle,
            kind: kindLabel,
            span: relationDaySpan(lagDays),
          })
        : m.gantt_arrow_label_lag_neg({
            from: fromTitle,
            to: toTitle,
            kind: kindLabel,
            span: relationDaySpan(Math.abs(lagDays)),
          });
  const parts = [base];
  if (late) parts.push(m.gantt_arrow_late());
  if (loop) parts.push(m.gantt_arrow_loop());
  return parts.join(' ');
}

/** One localized out-of-order sentence for cards sharing `(kind, lagDays)`. */
function lateSentence(kind: RelationKind, lagDays: number, cards: string): string {
  const span = relationDaySpan(Math.abs(lagDays));
  switch (kind) {
    case 'fs':
      if (lagDays === 0) return m.gantt_bar_late({ cards });
      return lagDays > 0
        ? m.gantt_bar_late_fs_pos({ cards, span })
        : m.gantt_bar_late_fs_neg({ cards, span });
    case 'ss':
      if (lagDays === 0) return m.gantt_bar_late_ss({ cards });
      return lagDays > 0
        ? m.gantt_bar_late_ss_pos({ cards, span })
        : m.gantt_bar_late_ss_neg({ cards, span });
    case 'ff':
      if (lagDays === 0) return m.gantt_bar_late_ff({ cards });
      return lagDays > 0
        ? m.gantt_bar_late_ff_pos({ cards, span })
        : m.gantt_bar_late_ff_neg({ cards, span });
    case 'sf':
      if (lagDays === 0) return m.gantt_bar_late_sf({ cards });
      return lagDays > 0
        ? m.gantt_bar_late_sf_pos({ cards, span })
        : m.gantt_bar_late_sf_neg({ cards, span });
  }
}

/**
 * Out-of-order sentences for a card's bar, grouped by `(kind, lagDays)` in
 * edge order so multiple plain `FS` predecessors still read as one sentence.
 */
export function barLateSentences(
  lateEdges: readonly RelationEdge[],
  titleOf: (cardId: string) => string | undefined,
  list: (titles: string[]) => string,
): string[] {
  const groups = new Map<string, { kind: RelationKind; lagDays: number; titles: string[] }>();
  for (const edge of lateEdges) {
    const title = titleOf(edge.from);
    if (title === undefined) continue;
    const key = `${edge.kind}:${String(edge.lagDays)}`;
    const group = groups.get(key);
    if (group) group.titles.push(title);
    else groups.set(key, { kind: edge.kind, lagDays: edge.lagDays, titles: [title] });
  }
  return [...groups.values()].map(({ kind, lagDays, titles }) =>
    lateSentence(kind, lagDays, list(titles)),
  );
}
