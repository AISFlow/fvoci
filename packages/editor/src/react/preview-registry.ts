import type { ReactNode } from "react";
import type { AttachmentMeta, PreviewAttachment } from "../attachment-model.js";

export {
	ATTACHMENT_ALIGNS,
	ATTACHMENT_WIDTH,
	type AttachmentAlign,
	type PreviewAttachment,
} from "../attachment-model.js";

export interface PreviewContext {
	downloadUrl: string;
	/** WHY: 치수 예약·배지는 첨부 메타에서 온다 — 브리지가 없으면 undefined. */
	meta?: () => Promise<AttachmentMeta | null>;
	/** WHY: 폭·정렬·캡션은 블록 prop — 읽기 전용이면 undefined. */
	updateProps?: (
		props: Partial<Omit<PreviewAttachment, "id" | "name" | "image">>,
	) => void;
}

export interface PreviewRenderer {
	canRender(att: PreviewAttachment): boolean;
	render(att: PreviewAttachment, ctx: PreviewContext): ReactNode;
}

const renderers: PreviewRenderer[] = [];

export function registerPreviewRenderer(renderer: PreviewRenderer): void {
	if (!renderers.includes(renderer)) renderers.push(renderer);
}

export function pickPreviewRenderer(
	att: PreviewAttachment,
): PreviewRenderer | null {
	return renderers.find((r) => r.canRender(att)) ?? null;
}

export function clearPreviewRenderers(): void {
	renderers.length = 0;
}
