// Adapted from packages/contracts/src/display-id.ts at source SHA
// 393795261322b916e588043cf94feca999175843.

/*
 * WHY: #807 B2 — `KEY-n`/`WIKI-n` 문법 정본. href·lookupDisplayId 와 같은 모양이며
 * 접두는 projectKeyPattern(2–32, 하이픈)과 맞춘다. 번호는 9자리로 잘라 정수 범위 안.
 */
export const displayIdPattern = /^([A-Za-z0-9-]{2,32})-(\d{1,9})$/;

export type ParsedDisplayId = { prefix: string; n: number };

export function parseDisplayId(raw: string): ParsedDisplayId | null {
	const match = displayIdPattern.exec(raw.trim());
	const prefix = match?.[1];
	const digits = match?.[2];
	if (prefix === undefined || digits === undefined) return null;
	return { prefix: prefix.toUpperCase(), n: Number(digits) };
}

export function formatDisplayId(prefix: string, n: number): string {
	return `${prefix}-${n}`;
}
