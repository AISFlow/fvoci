import type { GanttLink, LaidOutBar, LinkPath, Point } from "./types";

const ELBOW_PAD = 10;
const DETOUR_STEP = 6;

export function linkPaths(
	links: readonly GanttLink[],
	bars: readonly LaidOutBar[],
	laneHeight: number,
	pxPerDay: number,
): LinkPath[] {
	const byId = new Map<string, LaidOutBar>();
	for (const b of bars) byId.set(b.id, b);

	const items: {
		blockerId: string;
		blockedId: string;
		from: LaidOutBar;
		to: LaidOutBar;
		x1: number;
		x2: number;
		y1: number;
		y2: number;
	}[] = [];
	for (const l of links) {
		const from = byId.get(l.blockerId);
		const to = byId.get(l.blockedId);
		if (from === undefined || to === undefined) continue;
		const lag = l.lagDays * pxPerDay;
		let x1: number;
		let x2: number;
		if (l.type === "SS") {
			x1 = from.x;
			x2 = to.x + lag;
		} else if (l.type === "FF") {
			x1 = from.x + from.width;
			x2 = to.x + to.width + lag;
		} else {
			x1 = from.x + from.width;
			x2 = to.x + lag;
		}
		items.push({
			blockerId: l.blockerId,
			blockedId: l.blockedId,
			from,
			to,
			x1,
			x2,
			y1: from.lane * laneHeight + laneHeight / 2,
			y2: to.lane * laneHeight + laneHeight / 2,
		});
	}

	const used: { y: number; left: number; right: number }[] = [];
	return items.map((it) => ({
		blockerId: it.blockerId,
		blockedId: it.blockedId,
		points: elbow(it, bars, laneHeight, used),
	}));
}

function elbow(
	it: {
		from: LaidOutBar;
		to: LaidOutBar;
		x1: number;
		x2: number;
		y1: number;
		y2: number;
	},
	bars: readonly LaidOutBar[],
	laneHeight: number,
	used: { y: number; left: number; right: number }[],
): Point[] {
	const { from, to, x1, x2, y1, y2 } = it;
	const overlapL = Math.max(from.x, to.x);
	const overlapR = Math.min(from.x + from.width, to.x + to.width);
	if (overlapR > overlapL && y1 !== y2) {
		const cx = (overlapL + overlapR) / 2;
		const edge = laneHeight * 0.26;
		return [
			{ x: cx, y: y1 < y2 ? y1 + edge : y1 - edge },
			{ x: cx, y: y2 < y1 ? y2 + edge : y2 - edge },
		];
	}
	const gap = x2 - x1;
	if (gap >= ELBOW_PAD * 2) {
		const mid = (x1 + x2) / 2;
		return [
			{ x: x1, y: y1 },
			{ x: mid, y: y1 },
			{ x: mid, y: y2 },
			{ x: x2, y: y2 },
		];
	}
	if (gap >= 0) {
		if (y1 === y2) {
			return [
				{ x: x1, y: y1 },
				{ x: x2, y: y2 },
			];
		}
		const mid = (x1 + x2) / 2;
		return [
			{ x: x1, y: y1 },
			{ x: mid, y: y1 },
			{ x: mid, y: y2 },
			{ x: x2, y: y2 },
		];
	}
	const left = x2 - ELBOW_PAD;
	const right = x1 + ELBOW_PAD;
	const detourY = pickGutter(left, right, y1, y2, bars, laneHeight, used);
	return [
		{ x: x1, y: y1 },
		{ x: x1 + ELBOW_PAD, y: y1 },
		{ x: x1 + ELBOW_PAD, y: detourY },
		{ x: x2 - ELBOW_PAD, y: detourY },
		{ x: x2 - ELBOW_PAD, y: y2 },
		{ x: x2, y: y2 },
	];
}

function pickGutter(
	left: number,
	right: number,
	y1: number,
	y2: number,
	bars: readonly LaidOutBar[],
	laneHeight: number,
	used: { y: number; left: number; right: number }[],
): number {
	const lo = Math.min(y1, y2);
	const hi = Math.max(y1, y2);
	const prefs: number[] = [];
	if (y1 !== y2) prefs.push((y1 + y2) / 2);
	// WHY: 전 레인 후보는 edge×lane이라 월 뷰 500레인 우회가 수 분이 된다.
	const gutter = (Math.floor(lo / laneHeight) + 1) * laneHeight;
	if (gutter > lo && gutter < hi) prefs.push(gutter);
	prefs.push(lo - laneHeight * 0.38);
	prefs.push(hi + laneHeight * 0.38);

	const seen = new Set<number>();
	const deltas = [
		0,
		-DETOUR_STEP,
		DETOUR_STEP,
		-DETOUR_STEP * 2,
		DETOUR_STEP * 2,
		-DETOUR_STEP * 3,
		DETOUR_STEP * 3,
	];
	for (const base of prefs) {
		if (seen.has(base)) continue;
		seen.add(base);
		for (const d of deltas) {
			const y = base + d;
			if (y < 4) continue;
			if (hitsName(y, left, right, bars, laneHeight)) continue;
			if (
				used.some(
					(u) =>
						Math.abs(u.y - y) < DETOUR_STEP - 1 &&
						left < u.right &&
						right > u.left,
				)
			) {
				continue;
			}
			used.push({ y, left, right });
			return y;
		}
	}
	const fallback = hi + laneHeight * 0.38;
	used.push({ y: fallback, left, right });
	return fallback;
}

function hitsName(
	y: number,
	left: number,
	right: number,
	bars: readonly LaidOutBar[],
	laneHeight: number,
): boolean {
	const half = laneHeight * 0.32;
	for (const b of bars) {
		const cy = b.lane * laneHeight + laneHeight / 2;
		if (y < cy - half || y > cy + half) continue;
		const x0 = b.x;
		const x1 = b.x + b.width + (b.milestone ? 64 : 0);
		if (left < x1 && right > x0) return true;
	}
	return false;
}

export function toPolylinePoints(path: LinkPath): string {
	return path.points.map((p) => `${p.x},${p.y}`).join(" ");
}
