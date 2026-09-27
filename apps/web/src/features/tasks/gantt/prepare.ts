import { packFlow, stackRows } from "./layout";
import { linkPaths } from "./links";
import { monthBands, scaleWidth, ticks } from "./scale";
import { scheduleTasks } from "./schedule";
import type { GanttLink, GanttTask, TimeScale, WorkCalendar } from "./types";

type CompactLinkPath = [blocker: number, blocked: number, ...points: number[]];

/** WHY: 표시 polyline만 자른다. 일정·레인 계산은 전체 의존을 유지한다. */
const MAX_DISPLAY_PATHS = 2048;

export function prepareGantt({
	tasks,
	links = [],
	scale,
	calendar,
	laneHeight = 36,
	pack = "rows",
	maxLanes,
}: {
	tasks: readonly GanttTask[];
	links?: readonly GanttLink[];
	scale: TimeScale;
	calendar?: WorkCalendar;
	laneHeight?: number;
	pack?: "rows" | "overlap";
	maxLanes?: number;
}) {
	const { scheduled: items, dropped } = scheduleTasks(tasks);
	const layout =
		pack === "overlap"
			? packFlow(items, scale, links, maxLanes)
			: stackRows(items, scale);
	const barIds = new Set(layout.bars.map((bar) => bar.id));
	const drawable = links.filter(
		(link) => barIds.has(link.blockerId) && barIds.has(link.blockedId),
	);
	const shown =
		drawable.length <= MAX_DISPLAY_PATHS
			? drawable
			: drawable.toSorted(compareLinkIds).slice(0, MAX_DISPLAY_PATHS);
	const paths = linkPaths(shown, layout.bars, laneHeight, scale.pxPerDay);
	const itemIndex = new Map(items.map((item, index) => [item.id, index]));
	const compactPaths = paths.map((path): CompactLinkPath => {
		const blocker = itemIndex.get(path.blockerId);
		const blocked = itemIndex.get(path.blockedId);
		if (blocker === undefined || blocked === undefined)
			throw new Error("layout path references a missing item");
		return [
			blocker,
			blocked,
			...path.points.flatMap((point) => [point.x, point.y]),
		];
	});
	const columns = ticks(scale, calendar);
	const laneCount = layout.bars.reduce(
		(max, bar) => Math.max(max, bar.lane + 1),
		0,
	);
	const maxLinkY = paths.reduce(
		(max, path) => Math.max(max, ...path.points.map((point) => point.y)),
		0,
	);
	return {
		items,
		scale,
		laneHeight,
		laneCount,
		pack,
		bars: layout.bars.toSorted((a, b) => a.lane - b.lane || a.x - b.x),
		pathTotal: drawable.length,
		paths: compactPaths,
		columns,
		monthBands: monthBands(columns),
		width: scaleWidth(scale),
		height: Math.max(laneCount * laneHeight, maxLinkY + 12, laneHeight),
		overflow: layout.overflow,
		dropped,
	};
}

export type PreparedGantt = ReturnType<typeof prepareGantt>;

function compareLinkIds(a: GanttLink, b: GanttLink): number {
	if (a.blockerId !== b.blockerId) return a.blockerId < b.blockerId ? -1 : 1;
	if (a.blockedId === b.blockedId) return 0;
	return a.blockedId < b.blockedId ? -1 : 1;
}
