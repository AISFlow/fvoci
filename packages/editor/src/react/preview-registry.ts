import type { ReactNode } from "react";
import type { AttachmentMeta } from "./attachment-view.js";

export const ATTACHMENT_ALIGNS = ["left", "center", "right"] as const;
export type AttachmentAlign = (typeof ATTACHMENT_ALIGNS)[number];
export const ATTACHMENT_WIDTH = { min: 25, max: 100, step: 5 } as const;

export interface PreviewAttachment {
	id: string;
	name: string;
	image: boolean;
	width: number;
	align: AttachmentAlign;
	caption: string;
	/** WHY: 0 = 아직 모름 — 편집 세션이 메타 폴링으로 받아 블록에 적는다(C3 치수 예약). */
	previewWidth: number;
	previewHeight: number;
}

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
