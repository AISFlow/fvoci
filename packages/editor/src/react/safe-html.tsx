// packages/editor/src/react/safe-html.tsx
import type { ComponentProps } from "react";
import type { SafeHtml } from "../safe-html.js";

export { asSafeHtml, type SafeHtml } from "../safe-html.js";

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
