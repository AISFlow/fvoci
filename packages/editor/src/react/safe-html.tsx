// packages/editor/src/react/safe-html.tsx
import type { ComponentProps } from "react";

/** WHY: sanitize 가 끝난 HTML 만 이 타입을 얻는다 — 생산자 = asSafeHtml 호출자 전부(grep 으로 감사). */
export type SafeHtml = string & { readonly __brand: "SafeHtml" };

export function asSafeHtml(html: string): SafeHtml {
	return html as SafeHtml;
}

export function SafeHtmlView({
	html,
	...props
}: Omit<ComponentProps<"div">, "children" | "dangerouslySetInnerHTML"> & {
	html: SafeHtml;
}) {
	return (
		// biome-ignore lint/security/noDangerouslySetInnerHtml: 리포 유일 싱크 — 입력은 SafeHtml(katex trust:false · mermaid securityLevel:strict · 서버 sanitizeRenderedHtml)만
		<div {...props} dangerouslySetInnerHTML={{ __html: html }} />
	);
}
