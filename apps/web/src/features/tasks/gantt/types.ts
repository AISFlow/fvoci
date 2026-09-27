export type IsoDate = string;

export interface GanttTask {
	readonly id: string;
	readonly title: string;
	readonly start: IsoDate | null;
	readonly due: IsoDate | null;
	readonly milestone?: boolean;
	readonly parentId?: string | null;
}

export interface GanttLink {
	readonly blockerId: string;
	readonly blockedId: string;
	readonly type: "FS" | "SS" | "FF";
	readonly lagDays: number;
}

export interface ScheduledTask {
	readonly id: string;
	readonly title: string;
	readonly start: IsoDate;
	readonly end: IsoDate;
	readonly milestone: boolean;
	readonly inferred: ScheduleInference;
}

type ScheduleInference = "none" | "from-due" | "from-start" | "swapped";

export interface WorkCalendar {
	readonly weekend: readonly number[];
	readonly holidays: ReadonlySet<IsoDate>;
}

export type ZoomLevel = "day" | "week" | "month" | "quarter";

export interface TimeScale {
	readonly zoom: ZoomLevel;
	readonly start: IsoDate;
	readonly end: IsoDate;
	readonly pxPerDay: number;
}

export interface ScaleTick {
	readonly date: IsoDate;
	readonly x: number;
	readonly width: number;
	readonly label: string;
	readonly offDuty: boolean;
}

export interface LaidOutBar {
	readonly id: string;
	readonly lane: number;
	readonly x: number;
	readonly width: number;
	readonly milestone: boolean;
	readonly inferred: ScheduleInference;
}

export interface LinkPath {
	readonly blockerId: string;
	readonly blockedId: string;
	readonly points: readonly Point[];
}

export interface Point {
	readonly x: number;
	readonly y: number;
}
