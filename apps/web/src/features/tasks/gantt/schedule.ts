import { toEpochDay } from "./date";
import type { GanttTask, ScheduledTask } from "./types";

export function scheduleTasks(tasks: readonly GanttTask[]): {
	scheduled: ScheduledTask[];
	dropped: string[];
} {
	const scheduled: ScheduledTask[] = [];
	const dropped: string[] = [];
	for (const t of tasks) {
		const one = scheduleTask(t);
		if (one === null) dropped.push(t.id);
		else scheduled.push(one);
	}
	return { scheduled, dropped };
}

export function scheduleTask(t: GanttTask): ScheduledTask | null {
	const start = normalize(t.start);
	const due = normalize(t.due);

	if (start === null && due === null) return null;

	if (start !== null && due === null) {
		return build(t, start, start, "from-start");
	}
	if (start === null && due !== null) {
		return build(t, due, due, "from-due");
	}
	if (start === null || due === null) return null;

	const s = toEpochDay(start);
	const e = toEpochDay(due);
	if (s === null || e === null) return null;
	if (e < s) return build(t, due, start, "swapped");
	return build(t, start, due, "none");
}

function build(
	t: GanttTask,
	start: string,
	end: string,
	inferred: ScheduledTask["inferred"],
): ScheduledTask {
	return {
		id: t.id,
		title: t.title,
		start,
		end,
		milestone: t.milestone === true,
		inferred,
	};
}

function normalize(v: string | null | undefined): string | null {
	if (typeof v !== "string" || v === "") return null;
	return toEpochDay(v) === null ? null : v;
}
