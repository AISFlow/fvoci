// packages/ui/presence.ts

/*
 * WHY: #641 — 프레즌스 색은 서버가 세션 사용자 id 로 되계산해 덮어쓴다. 클라이언트가 실어 보낸
 * 색을 믿지 않으려면 서버와 웹이 같은 순수 함수를 봐야 해서 토큰과 같이 산다.
 * hex 는 apps/web/src/index.css 의 .afn-label-* --afn-label-ink 와 같은 값이고
 * apps/web/test/collab-session.test.ts 가 두 벌의 드리프트를 잠근다.
 */
export const PRESENCE_COLORS = [
	"#b91c1c",
	"#c2410c",
	"#b45309",
	"#15803d",
	"#0f766e",
	"#1d4ed8",
	"#6d28d9",
	"#be185d",
] as const;

export function presenceColorOf(userId: string): string {
	const tail = userId.replaceAll("-", "").slice(-6);
	const parsed = Number.parseInt(tail, 16);
	const index = Number.isFinite(parsed) ? parsed % PRESENCE_COLORS.length : 0;
	return PRESENCE_COLORS[index] ?? PRESENCE_COLORS[0];
}
