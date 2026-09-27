export function ganttBarPatch(
	task: {
		startDate: string | null;
		dueDate: string | null;
		dueAt: string | null;
	},
	next: { start: string; end: string },
): { startDate: string; dueDate: string; dueAt?: null } {
	const patch: { startDate: string; dueDate: string; dueAt?: null } = {
		startDate: next.start,
		dueDate: next.end,
	};
	if (task.dueAt !== null && task.dueDate === null) patch.dueAt = null;
	return patch;
}
