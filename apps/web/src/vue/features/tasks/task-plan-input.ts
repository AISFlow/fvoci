import { parseItemRef } from "@/lib/href";

export const planPresets = [
  { id: "study", label: "학습 계획", steps: ["자료 읽기", "핵심 내용 정리", "복습하기"] },
  { id: "research", label: "연구 계획", steps: ["자료 조사", "질문 검토", "분석·노트 정리"] },
  { id: "work", label: "업무 계획", steps: ["준비하기", "실행하기", "결과 검토"] },
] as const;

export function explicitMinutes(
  value: string,
): { valid: true; minutes: number | null } | { valid: false } {
  const raw = value.trim();
  if (!raw) return { valid: true, minutes: null };
  if (!/^\d+$/.test(raw)) return { valid: false };
  const minutes = Number(raw);
  return Number.isInteger(minutes) && minutes >= 0 && minutes <= 2147483647
    ? { valid: true, minutes }
    : { valid: false };
}

/** A user-selected ordinary document, never another workspace or remote URL. */
export function planDocumentRef(
  value: string,
  slug: string,
  origin: string,
): { displayId: string; anchor: string | null } | undefined {
  const raw = value.trim();
  const direct = parseItemRef(raw);
  if (direct) return { displayId: direct.displayId, anchor: null };
  try {
    const url = new URL(raw, origin);
    if (url.origin !== origin || url.username || url.password || url.search) return undefined;
    const parts = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
    if (parts.length !== 3 || parts[0] !== "w" || parts[1] !== slug) return undefined;
    const ref = parseItemRef(parts[2] ?? "");
    const anchor = url.hash ? decodeURIComponent(url.hash.slice(1)) : null;
    if (!ref || (anchor !== null && Array.from(anchor).length > 200)) return undefined;
    return { displayId: ref.displayId, anchor };
  } catch {
    return undefined;
  }
}

/** Explicit persisted unit is required; old numeric values are not time. */
export function minuteEstimate(value: string | null, unit: string | null): number | null {
  if (unit !== "minutes" || value === null) return null;
  const minutes = Number(value);
  return Number.isInteger(minutes) && minutes >= 0 && minutes <= 2147483647 ? minutes : null;
}
