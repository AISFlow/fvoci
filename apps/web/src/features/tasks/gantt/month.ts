import { addDays, dayOfWeek, toEpochDay } from "./date";
import type { IsoDate } from "./types";

/** 한 달을 덮는 주 단위 정렬 범위. 앞뒤로 주 경계까지 채운다. */
export function monthRange(
	year: number,
	month: number,
	opts: { weekStartsOn?: number } = {},
): { start: IsoDate; end: IsoDate } | null {
	const weekStartsOn = opts.weekStartsOn ?? 0;
	const first = `${String(year).padStart(4, "0")}-${String(month).padStart(2, "0")}-01`;
	const firstDay = toEpochDay(first);
	if (firstDay === null || month < 1 || month > 12) return null;

	const firstDow = dayOfWeek(first);
	if (firstDow === null) return null;
	const lead = (((firstDow - weekStartsOn) % 7) + 7) % 7;
	const start = addDays(first, -lead);
	if (start === null) return null;

	const nextMonthFirst =
		month === 12
			? `${String(year + 1).padStart(4, "0")}-01-01`
			: `${String(year).padStart(4, "0")}-${String(month + 1).padStart(2, "0")}-01`;
	const lastDay = toEpochDay(nextMonthFirst);
	if (lastDay === null) return null;
	const days = Math.ceil((lastDay - (firstDay - lead)) / 7) * 7;
	const end = addDays(start, days - 1);
	if (end === null) return null;

	return { start, end };
}
