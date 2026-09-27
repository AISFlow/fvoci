import { t } from "@fvoci/i18n";
import { isWorkingDay } from "./calendar";
import {
	addDays,
	daysBetween,
	eachDay,
	fromEpochDay,
	toEpochDay,
} from "./date";
import type {
	IsoDate,
	ScaleTick,
	TimeScale,
	WorkCalendar,
	ZoomLevel,
} from "./types";

const DEFAULT_PX_PER_DAY: Readonly<Record<ZoomLevel, number>> = {
	day: 32,
	week: 12,
	month: 4,
	quarter: 1.5,
};

export function makeScale(
	start: IsoDate,
	end: IsoDate,
	zoom: ZoomLevel,
	pxPerDay: number = DEFAULT_PX_PER_DAY[zoom],
): TimeScale {
	const span = daysBetween(start, end);
	const safeEnd = span === null || span < 0 ? start : end;
	return { zoom, start, end: safeEnd, pxPerDay };
}

export function dateToX(d: IsoDate, scale: TimeScale): number | null {
	const offset = daysBetween(scale.start, d);
	if (offset === null) return null;
	return offset * scale.pxPerDay;
}

export function xToDate(x: number, scale: TimeScale): IsoDate {
	const days = scale.pxPerDay <= 0 ? 0 : Math.floor(x / scale.pxPerDay);
	const raw = addDays(scale.start, days) ?? scale.start;
	if (raw < scale.start) return scale.start;
	if (raw > scale.end) return scale.end;
	return raw;
}

export function scaleWidth(scale: TimeScale): number {
	const span = daysBetween(scale.start, scale.end);
	if (span === null) return 0;
	return (span + 1) * scale.pxPerDay;
}

export function ticks(scale: TimeScale, cal?: WorkCalendar): ScaleTick[] {
	const startDay = toEpochDay(scale.start);
	const endDay = toEpochDay(scale.end);
	if (startDay === null || endDay === null || endDay < startDay) return [];

	const out: ScaleTick[] = [];
	let cursor = startDay;
	while (cursor <= endDay) {
		const date = fromEpochDay(cursor);
		const nextBoundary = nextTickStart(date, scale.zoom);
		const boundaryDay =
			nextBoundary === null ? endDay + 1 : toEpochDay(nextBoundary);
		const stop =
			boundaryDay === null ? endDay + 1 : Math.min(boundaryDay, endDay + 1);
		const days = stop - cursor;
		if (days <= 0) break;
		const x = (cursor - startDay) * scale.pxPerDay;
		out.push({
			date,
			x,
			width: days * scale.pxPerDay,
			label: tickLabel(date, scale.zoom),
			offDuty: cal !== undefined && isAllOffDuty(date, days, cal),
		});
		cursor = stop;
	}
	return out;
}

interface MonthBand {
	readonly key: string;
	readonly label: string;
	readonly x: number;
	readonly width: number;
}

export function monthBands(cols: readonly ScaleTick[]): MonthBand[] {
	const out: MonthBand[] = [];
	for (const c of cols) {
		const key = c.date.slice(0, 7);
		const last = out[out.length - 1];
		if (last && last.key === key) {
			out[out.length - 1] = {
				...last,
				width: last.width + c.width,
			};
			continue;
		}
		const month = Number(c.date.slice(5, 7));
		out.push({
			key,
			label: Number.isInteger(month) ? t("gantt.month", { month }) : key,
			x: c.x,
			width: c.width,
		});
	}
	return out;
}

function nextTickStart(d: IsoDate, zoom: ZoomLevel): IsoDate | null {
	if (zoom === "day") return addDays(d, 1);
	if (zoom === "week") {
		const dow = dayOfWeekOrZero(d);
		const toMonday = dow === 0 ? 1 : 8 - dow;
		return addDays(d, toMonday);
	}
	const [y, m] = splitYm(d);
	if (y === null || m === null) return null;
	const step = zoom === "month" ? 1 : 3 - ((m - 1) % 3);
	const nm = m + step;
	const ny = y + Math.floor((nm - 1) / 12);
	const nmm = ((nm - 1) % 12) + 1;
	return `${String(ny).padStart(4, "0")}-${String(nmm).padStart(2, "0")}-01`;
}

function isAllOffDuty(
	start: IsoDate,
	days: number,
	cal: WorkCalendar,
): boolean {
	const end = addDays(start, days - 1);
	if (end === null) return false;
	const all = eachDay(start, end);
	return all.length > 0 && all.every((d) => !isWorkingDay(d, cal));
}

function tickLabel(d: IsoDate, zoom: ZoomLevel): string {
	const [y, m] = splitYm(d);
	const day = d.slice(8, 10);
	if (zoom === "day") return String(Number(day));
	if (zoom === "week") return `${d.slice(5, 7)}/${day}`;
	if (zoom === "month") return `${y}-${String(m).padStart(2, "0")}`;
	const q = m === null ? 0 : Math.floor((m - 1) / 3) + 1;
	return `${y} Q${q}`;
}

function splitYm(d: IsoDate): [number | null, number | null] {
	const y = Number(d.slice(0, 4));
	const m = Number(d.slice(5, 7));
	if (!Number.isInteger(y) || !Number.isInteger(m)) return [null, null];
	return [y, m];
}

function dayOfWeekOrZero(d: IsoDate): number {
	const day = toEpochDay(d);
	if (day === null) return 0;
	return (((day + 4) % 7) + 7) % 7;
}
