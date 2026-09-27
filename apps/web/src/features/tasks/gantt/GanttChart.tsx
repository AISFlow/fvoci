// packages/gantt/src/react/GanttChart.tsx

import { t } from "@fvoci/i18n";
import type React from "react";
import { useId, useRef, useState } from "react";
import { addDays, daysBetween } from "./date";
import { barRect, laneCenterY } from "./layout";
import type { PreparedGantt } from "./prepare";
import { dateToX, xToDate } from "./scale";
import type {
	IsoDate,
	LaidOutBar,
	ScheduledTask,
	TimeScale,
} from "./types";

export interface GanttChartProps {
	readonly layout: PreparedGantt;
	readonly preview?: { id: string; start: IsoDate; end: IsoDate };
	readonly className?: string;
	readonly selectedId?: string;
	readonly today?: IsoDate;
	readonly showRail?: boolean;
	readonly renderRailRow?: (taskId: string) => React.ReactNode;
	readonly onSelect?: (taskId: string) => void;
	readonly onBarChange?: (p: {
		id: string;
		start: IsoDate;
		end: IsoDate;
	}) => void;
}

export function GanttChart({
	layout,
	preview,
	className,
	selectedId,
	today,
	showRail = true,
	renderRailRow,
	onSelect,
	onBarChange,
}: GanttChartProps) {
	const uid = useId().replace(/:/g, "");
	const markerId = `fvoci-gantt-arrow-${uid}`;
	const svgRef = useRef<SVGSVGElement>(null);
	const dragRef = useRef<BarDrag | null>(null);
	const skipSelectRef = useRef(false);
	const [live, setLive] = useState<{
		id: string;
		start: IsoDate;
		end: IsoDate;
	} | null>(null);
	const {
		items: scheduled,
		bars,
		pathTotal,
		paths,
		columns: cols,
		monthBands: months,
		width,
		height,
		overflow,
		scale,
		laneHeight,
		laneCount,
		pack,
	} = layout;
	const titleById = new Map(scheduled.map((s) => [s.id, s.title]));
	const todayDate = today ?? new Date().toISOString().slice(0, 10);
	const todayX = dateToX(todayDate, scale);
	const showToday = todayX !== null && todayX >= 0 && todayX <= width;
	const ordered = bars;
	const scheduledById = new Map(scheduled.map((s) => [s.id, s]));
	const displayBars = bars.map((b) =>
		overlayBar(b, scheduledById.get(b.id), live ?? preview ?? null, scale),
	);
	const editable = onBarChange !== undefined;

	const clientToX = (clientX: number): number => {
		const svg = svgRef.current;
		if (!svg) return 0;
		const box = svg.getBoundingClientRect();
		if (box.width <= 0) return 0;
		return ((clientX - box.left) / box.width) * width;
	};

	const beginDrag = (e: React.PointerEvent, id: string, kind: BarDragKind) => {
		if (!editable || e.button !== 0) return;
		const sched = scheduledById.get(id);
		if (!sched) return;
		e.stopPropagation();
		svgRef.current?.setPointerCapture(e.pointerId);
		const originX = clientToX(e.clientX);
		dragRef.current = {
			id,
			kind,
			originStart: sched.start,
			originEnd: sched.end,
			originX,
			originClientX: e.clientX,
			start: sched.start,
			end: sched.end,
			moved: false,
		};
	};

	const onCanvasPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
		const drag = dragRef.current;
		if (!drag) return;
		if (Math.abs(e.clientX - drag.originClientX) >= SELECT_SLOP) {
			drag.moved = true;
		}
		if (!drag.moved) return;
		const next = applyBarPointer(drag, clientToX(e.clientX), scale);
		drag.start = next.start;
		drag.end = next.end;
		setLive({ id: drag.id, start: next.start, end: next.end });
	};

	const onCanvasPointerUp = (e: React.PointerEvent<SVGSVGElement>) => {
		const drag = dragRef.current;
		dragRef.current = null;
		if (svgRef.current?.hasPointerCapture(e.pointerId)) {
			svgRef.current.releasePointerCapture(e.pointerId);
		}
		setLive(null);
		if (!drag) return;
		if (
			drag.moved ||
			drag.start !== drag.originStart ||
			drag.end !== drag.originEnd
		) {
			skipSelectRef.current = true;
			window.setTimeout(() => {
				skipSelectRef.current = false;
			}, 0);
		}
		if (
			onBarChange !== undefined &&
			(drag.start !== drag.originStart || drag.end !== drag.originEnd)
		) {
			onBarChange({ id: drag.id, start: drag.start, end: drag.end });
		}
	};

	return (
		<div
			className={[
				"fvoci-gantt",
				showRail ? "fvoci-gantt--split" : "",
				className,
			]
				.filter(Boolean)
				.join(" ")}
			data-slot="gantt"
			data-overflow={overflow.length}
			data-pack={pack}
			data-path-total={pathTotal}
			data-path-count={paths.length}
			data-bar-edit={editable ? "1" : undefined}
		>
			{showRail ? (
				<div className="fvoci-gantt__rail">
					<div className="fvoci-gantt__rail-head">{t("gantt.rail")}</div>
					{ordered.map((b) => {
						const title = titleById.get(b.id) ?? b.id;
						const selected = selectedId === b.id;
						return (
							<button
								key={b.id}
								type="button"
								className={[
									"fvoci-gantt__row-label",
									selected ? "fvoci-gantt__row-label--selected" : "",
									b.inferred === "none" ? "" : "fvoci-gantt__bar--inferred",
								]
									.filter(Boolean)
									.join(" ")}
								style={{ height: laneHeight }}
								aria-pressed={selected}
								onClick={
									onSelect === undefined ? undefined : () => onSelect(b.id)
								}
							>
								{renderRailRow ? renderRailRow(b.id) : title}
							</button>
						);
					})}
				</div>
			) : null}
			{/* biome-ignore lint/a11y/noNoninteractiveTabindex: Native keyboard scrolling needs a focusable scroll region. */}
			<section className="fvoci-gantt__board" aria-label="Gantt" tabIndex={0}>
				<div className="fvoci-gantt__header" style={{ width }}>
					{months.map((m) => (
						<span
							key={m.key}
							className="fvoci-gantt__month"
							style={{ left: m.x, width: m.width }}
						>
							{m.label}
						</span>
					))}
					{cols.map((c) => (
						<span
							key={c.date}
							className={[
								"fvoci-gantt__tick",
								c.offDuty ? "fvoci-gantt__col--offduty" : "",
								c.date === todayDate ? "fvoci-gantt__day--today" : "",
							]
								.filter(Boolean)
								.join(" ")}
							style={{ left: c.x, width: c.width }}
						>
							<span
								className={
									c.date === todayDate
										? "fvoci-gantt__tick-today"
										: "fvoci-gantt__tick-label"
								}
							>
								{c.label}
							</span>
						</span>
					))}
				</div>
				<svg
					ref={svgRef}
					className="fvoci-gantt__canvas"
					width={width}
					height={height}
					overflow="visible"
					aria-label={t("gantt.chart.aria", {
						count: bars.length,
						start: scale.start,
						end: scale.end,
					})}
					onPointerMove={editable ? onCanvasPointerMove : undefined}
					onPointerUp={editable ? onCanvasPointerUp : undefined}
					onPointerCancel={editable ? onCanvasPointerUp : undefined}
				>
					<title>
						{t("gantt.chart.title", {
							start: scale.start,
							end: scale.end,
						})}
					</title>
					<defs>
						<marker
							id={markerId}
							markerWidth="8"
							markerHeight="8"
							refX="7"
							refY="4"
							orient="auto"
							markerUnits="userSpaceOnUse"
						>
							<path d="M0 0 L8 4 L0 8 z" className="fvoci-gantt__link-head" />
						</marker>
					</defs>
					{cols
						.filter((c) => c.offDuty)
						.map((c) => (
							<rect
								key={`off-${c.date}`}
								className="fvoci-gantt__col--offduty"
								x={c.x}
								y={0}
								width={c.width}
								height={height}
								pointerEvents="none"
							/>
						))}
					{Array.from(
						{ length: Math.max(laneCount, 1) },
						(_, n) => (n + 1) * laneHeight,
					).map((y) => (
						<line
							key={y}
							className="fvoci-gantt__row-rule"
							x1={0}
							x2={width}
							y1={y}
							y2={y}
							pointerEvents="none"
						/>
					))}
					{showToday && todayX !== null ? (
						<>
							<rect
								className="fvoci-gantt__today-band"
								x={todayX}
								y={0}
								width={Math.max(scale.pxPerDay, 2)}
								height={height}
								pointerEvents="none"
							/>
							<line
								className="fvoci-gantt__today-line"
								x1={todayX}
								x2={todayX}
								y1={0}
								y2={height}
								pointerEvents="none"
							/>
						</>
					) : null}
					{displayBars.map((b) => {
						const cy = laneCenterY(b.lane, laneHeight);
						const barHeight = laneHeight * (pack === "overlap" ? 0.52 : 0.58);
						const title = titleById.get(b.id) ?? b.id;
						const selected = selectedId === b.id;
						const place = labelPlacement(b, displayBars, title);
						const innerLabel = place === "inner";
						const clipId = `${markerId}-clip-${b.id}`;
						const bw = Math.max(b.width, 2);
						const barY = cy - barHeight / 2;
						return (
							<g
								key={b.id}
								className={[
									"fvoci-gantt__bar",
									b.inferred === "none" ? "" : "fvoci-gantt__bar--inferred",
									selected ? "fvoci-gantt__bar--selected" : "",
									live?.id === b.id ? "fvoci-gantt__bar--live" : "",
								]
									.filter(Boolean)
									.join(" ")}
								{...(onSelect === undefined
									? {}
									: {
											role: "button" as const,
											tabIndex: 0,
											"aria-label": title,
											"aria-pressed": selected,
											onClick: () => {
												if (skipSelectRef.current) {
													skipSelectRef.current = false;
													return;
												}
												onSelect(b.id);
											},
											onKeyDown: (e: React.KeyboardEvent<SVGGElement>) => {
												if (e.key === "Enter" || e.key === " ") {
													e.preventDefault();
													onSelect(b.id);
												}
											},
										})}
							>
								<title>{`${title} (${b.milestone ? t("gantt.milestone") : t("gantt.kind.bar")})`}</title>
								{pack === "rows" ? (
									<rect
										className="fvoci-gantt__hit"
										x={0}
										y={b.lane * laneHeight}
										width={width}
										height={laneHeight}
										fill="transparent"
									/>
								) : null}
								{innerLabel ? (
									<clipPath id={clipId}>
										<rect
											x={b.x}
											y={barY}
											width={bw}
											height={barHeight}
											rx={barHeight / 2}
										/>
									</clipPath>
								) : null}
								{b.milestone ? (
									<polygon
										className="fvoci-gantt__milestone"
										points={diamond(b.x + b.width / 2, cy, b.width / 2)}
										onPointerDown={
											editable ? (e) => beginDrag(e, b.id, "move") : undefined
										}
									/>
								) : (
									<rect
										className="fvoci-gantt__bar-rect"
										x={b.x}
										y={barY}
										width={bw}
										height={barHeight}
										rx={barHeight / 2}
										onPointerDown={
											editable ? (e) => beginDrag(e, b.id, "move") : undefined
										}
									/>
								)}
								{editable && !b.milestone ? (
									<>
										<rect
											className="fvoci-gantt__handle"
											x={b.x}
											y={barY}
											width={HANDLE_PX}
											height={barHeight}
											onPointerDown={(e) => beginDrag(e, b.id, "start")}
										/>
										<rect
											className="fvoci-gantt__handle"
											x={b.x + bw - HANDLE_PX}
											y={barY}
											width={HANDLE_PX}
											height={barHeight}
											onPointerDown={(e) => beginDrag(e, b.id, "end")}
										/>
									</>
								) : null}
							</g>
						);
					})}
					{paths.map((p) => (
						<polyline
							key={`${p[0]}->${p[1]}`}
							className="fvoci-gantt__link"
							points={p.slice(2).join(" ")}
							fill="none"
							markerEnd={`url(#${markerId})`}
							pointerEvents="none"
						/>
					))}
					{displayBars.map((b) => {
						const cy = laneCenterY(b.lane, laneHeight);
						const title = titleById.get(b.id) ?? b.id;
						const place = labelPlacement(b, displayBars, title);
						if (place === "none") return null;
						const innerLabel = place === "inner";
						const clipId = `${markerId}-clip-${b.id}`;
						return (
							<text
								key={`label-${b.id}`}
								className={
									innerLabel
										? "fvoci-gantt__bar-label"
										: "fvoci-gantt__bar-label fvoci-gantt__bar-label--outside"
								}
								clipPath={innerLabel ? `url(#${clipId})` : undefined}
								x={innerLabel ? b.x + 8 : b.x + b.width + 8}
								y={cy}
								dy="0.35em"
								pointerEvents="none"
							>
								{title}
							</text>
						);
					})}
				</svg>
			</section>
		</div>
	);
}

function diamond(cx: number, cy: number, r: number): string {
	return `${cx},${cy - r} ${cx + r},${cy} ${cx},${cy + r} ${cx - r},${cy}`;
}

const CHAR_PX = 7;
const HANDLE_PX = 8;
const SELECT_SLOP = 4;

type BarDragKind = "move" | "start" | "end";

interface BarDrag {
	id: string;
	kind: BarDragKind;
	originStart: IsoDate;
	originEnd: IsoDate;
	originX: number;
	originClientX: number;
	start: IsoDate;
	end: IsoDate;
	moved: boolean;
}

function clampIso(d: IsoDate, start: IsoDate, end: IsoDate): IsoDate {
	if (d < start) return start;
	if (d > end) return end;
	return d;
}

function applyBarPointer(
	drag: Pick<BarDrag, "kind" | "originStart" | "originEnd" | "originX">,
	x: number,
	scale: TimeScale,
): { start: IsoDate; end: IsoDate } {
	if (drag.kind === "move") {
		const delta =
			daysBetween(xToDate(drag.originX, scale), xToDate(x, scale)) ?? 0;
		const span = daysBetween(drag.originStart, drag.originEnd) ?? 0;
		let start = addDays(drag.originStart, delta) ?? drag.originStart;
		start = clampIso(start, scale.start, scale.end);
		let end = addDays(start, span) ?? start;
		if (end > scale.end) {
			end = scale.end;
			start = addDays(end, -span) ?? scale.start;
			start = clampIso(start, scale.start, scale.end);
		}
		if (end < start) end = start;
		return { start, end };
	}
	if (drag.kind === "start") {
		let start = xToDate(x, scale);
		if (start > drag.originEnd) start = drag.originEnd;
		return { start, end: drag.originEnd };
	}
	let end = xToDate(x, scale);
	if (end < drag.originStart) end = drag.originStart;
	return { start: drag.originStart, end };
}

function overlayBar(
	b: LaidOutBar,
	sched: ScheduledTask | undefined,
	live: { id: string; start: IsoDate; end: IsoDate } | null,
	scale: TimeScale,
): LaidOutBar {
	if (!live || live.id !== b.id || !sched) return b;
	const rect = barRect({ ...sched, start: live.start, end: live.end }, scale);
	if (!rect) return b;
	return { ...b, x: rect.x, width: rect.width };
}

function labelPlacement(
	b: { id: string; x: number; width: number; lane: number; milestone: boolean },
	bars: readonly { id: string; x: number; width: number; lane: number }[],
	title: string,
): "inner" | "outer" | "none" {
	if (!b.milestone && b.width >= 56) return "inner";
	const start = b.x + b.width + 8;
	const end = start + Math.min(title.length * CHAR_PX, 96);
	const hits = bars.some(
		(o) =>
			o.id !== b.id && o.lane === b.lane && o.x < end && o.x + o.width > start,
	);
	if (hits) return !b.milestone && b.width >= 32 ? "inner" : "none";
	return "outer";
}
