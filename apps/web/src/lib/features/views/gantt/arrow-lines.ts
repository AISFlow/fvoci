/**
 * Builds drawn `ArrowLine` view-models from `ChartRelations` and `bars`.
 */
import { arrowAccessibleLabel, relationBadge } from '$lib/components/properties/relation-labels.ts';
import {
  arrowHead,
  arrowHitPoints,
  arrowLabelPoint,
  arrowPath,
  arrowsOf,
  edgeKey,
  type Bar,
  type ChartRelations,
} from '$lib/logic/gantt.ts';
import type { RelationKind } from '$lib/logic/relations.ts';

/** An arrow as drawn: its ends, kind/lag, marks, SVG paths, badge point, and accessible label. */
export interface ArrowLine {
  key: string;
  from: string;
  to: string;
  kind: RelationKind;
  lagDays: number;
  late: boolean;
  loop: boolean;
  path: string;
  head: string;
  hitPath: string;
  labelX: number;
  labelY: number;
  badge: string;
  label: string;
  fromTitle: string;
  toTitle: string;
}

/** Converts chart relations and bar boxes into `ArrowLine`s ready for SVG/overlay rendering. */
export function buildArrowLines(
  relations: ChartRelations,
  bars: ReadonlyMap<string, Bar>,
  axisWidth: number,
  titleOf: (cardId: string) => string,
): ArrowLine[] {
  return arrowsOf(relations, bars, axisWidth).map((arrow) => {
    const fromTitle = titleOf(arrow.from);
    const toTitle = titleOf(arrow.to);
    const [labelX, labelY] = arrowLabelPoint(arrow.points);
    return {
      key: edgeKey(arrow),
      from: arrow.from,
      to: arrow.to,
      kind: arrow.kind,
      lagDays: arrow.lagDays,
      late: arrow.late,
      loop: arrow.loop,
      path: arrowPath(arrow.points),
      head: arrowHead(arrow.points, arrow.kind),
      hitPath: arrowPath(arrowHitPoints(arrow.points)),
      labelX,
      labelY,
      badge: relationBadge(arrow.kind, arrow.lagDays),
      label: arrowAccessibleLabel(
        fromTitle,
        toTitle,
        arrow.kind,
        arrow.lagDays,
        arrow.late,
        arrow.loop,
      ),
      fromTitle,
      toTitle,
    };
  });
}
