import { daysBetween } from "./date";
import { dateToX } from "./scale";
import type {
	GanttLink,
	LaidOutBar,
	ScheduledTask,
	TimeScale,
} from "./types";

const MILESTONE_WIDTH = 12;
const PACK_GAP = 1;

export function packLanes(
	tasks: readonly ScheduledTask[],
	scale: TimeScale,
	maxLanes = Number.POSITIVE_INFINITY,
): { bars: LaidOutBar[]; overflow: string[] } {
	const sorted = tasks.toSorted(compareForPacking);
	const laneEnd: number[] = [];
	const bars: LaidOutBar[] = [];
	const overflow: string[] = [];

	for (const t of sorted) {
		const rect = barRect(t, scale);
		if (rect === null) {
			overflow.push(t.id);
			continue;
		}
		const lane = firstFreeLane(laneEnd, rect.x);
		if (lane >= maxLanes) {
			overflow.push(t.id);
			continue;
		}
		laneEnd[lane] = rect.x + rect.width;
		bars.push({
			id: t.id,
			lane,
			x: rect.x,
			width: rect.width,
			milestone: t.milestone,
			inferred: t.inferred,
		});
	}
	return { bars, overflow };
}

export function packFlow(
	tasks: readonly ScheduledTask[],
	scale: TimeScale,
	links: readonly GanttLink[] = [],
	maxLanes = Number.POSITIVE_INFINITY,
): { bars: LaidOutBar[]; overflow: string[] } {
	const order = orderByDependency(tasks, links);
	const preds = new Map<string, string[]>();
	for (const l of links) {
		const arr = preds.get(l.blockedId) ?? [];
		arr.push(l.blockerId);
		preds.set(l.blockedId, arr);
	}
	const laneEnd: number[] = [];
	const placed = new Map<string, LaidOutBar>();
	const bars: LaidOutBar[] = [];
	const overflow: string[] = [];

	for (const t of order) {
		const rect = barRect(t, scale);
		if (rect === null) {
			overflow.push(t.id);
			continue;
		}
		let minLane = 0;
		for (const pid of preds.get(t.id) ?? []) {
			const pb = placed.get(pid);
			if (pb === undefined) continue;
			if (rangesOverlap(pb, rect)) minLane = Math.max(minLane, pb.lane + 1);
			else minLane = Math.max(minLane, pb.lane);
		}
		const lane = firstFreeLane(laneEnd, rect.x, minLane);
		if (lane >= maxLanes) {
			overflow.push(t.id);
			continue;
		}
		laneEnd[lane] = rect.x + rect.width;
		const bar: LaidOutBar = {
			id: t.id,
			lane,
			x: rect.x,
			width: rect.width,
			milestone: t.milestone,
			inferred: t.inferred,
		};
		placed.set(t.id, bar);
		bars.push(bar);
	}
	return { bars, overflow };
}

export function stackRows(
	tasks: readonly ScheduledTask[],
	scale: TimeScale,
): { bars: LaidOutBar[]; overflow: string[] } {
	const sorted = tasks.toSorted(compareForPacking);
	const bars: LaidOutBar[] = [];
	const overflow: string[] = [];
	let lane = 0;
	for (const t of sorted) {
		const rect = barRect(t, scale);
		if (rect === null) {
			overflow.push(t.id);
			continue;
		}
		bars.push({
			id: t.id,
			lane,
			x: rect.x,
			width: rect.width,
			milestone: t.milestone,
			inferred: t.inferred,
		});
		lane += 1;
	}
	return { bars, overflow };
}

export function barRect(
	t: ScheduledTask,
	scale: TimeScale,
): { x: number; width: number } | null {
	const x = dateToX(t.start, scale);
	if (x === null) return null;
	if (t.milestone) {
		return {
			x: x + scale.pxPerDay / 2 - MILESTONE_WIDTH / 2,
			width: MILESTONE_WIDTH,
		};
	}
	const span = daysBetween(t.start, t.end);
	if (span === null) return null;
	return { x, width: (span + 1) * scale.pxPerDay };
}

export function laneCenterY(lane: number, laneHeight: number): number {
	return lane * laneHeight + laneHeight / 2;
}

function firstFreeLane(
	laneEnd: readonly number[],
	x: number,
	minLane = 0,
): number {
	for (let i = minLane; i < laneEnd.length; i++) {
		const end = laneEnd[i];
		if (end === undefined || end + PACK_GAP <= x) return i;
	}
	return Math.max(laneEnd.length, minLane);
}

function rangesOverlap(
	a: { x: number; width: number },
	b: { x: number; width: number },
): boolean {
	return a.x < b.x + b.width + PACK_GAP && b.x < a.x + a.width + PACK_GAP;
}

function orderByDependency(
	tasks: readonly ScheduledTask[],
	links: readonly GanttLink[],
): ScheduledTask[] {
	const byId = new Map(tasks.map((t) => [t.id, t]));
	const indeg = new Map(tasks.map((t) => [t.id, 0]));
	const adj = new Map(tasks.map((t) => [t.id, [] as string[]]));
	for (const l of links) {
		if (!byId.has(l.blockerId) || !byId.has(l.blockedId)) continue;
		adj.get(l.blockerId)?.push(l.blockedId);
		indeg.set(l.blockedId, (indeg.get(l.blockedId) ?? 0) + 1);
	}
	const ready = tasks
		.filter((t) => (indeg.get(t.id) ?? 0) === 0)
		.toSorted(compareForPacking);
	const out: ScheduledTask[] = [];
	const deg = new Map(indeg);
	while (ready.length > 0) {
		const t = ready.shift();
		if (t === undefined) break;
		out.push(t);
		for (const n of adj.get(t.id) ?? []) {
			const next = (deg.get(n) ?? 1) - 1;
			deg.set(n, next);
			if (next === 0) {
				const nt = byId.get(n);
				if (nt !== undefined) {
					ready.push(nt);
					ready.sort(compareForPacking);
				}
			}
		}
	}
	if (out.length < tasks.length) {
		const seen = new Set(out.map((t) => t.id));
		out.push(
			...tasks.filter((t) => !seen.has(t.id)).toSorted(compareForPacking),
		);
	}
	return out;
}

function compareForPacking(a: ScheduledTask, b: ScheduledTask): number {
	if (a.start !== b.start) return a.start < b.start ? -1 : 1;
	if (a.id === b.id) return 0;
	return a.id < b.id ? -1 : 1;
}
