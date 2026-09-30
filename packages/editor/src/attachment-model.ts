import { formatBytes } from "./format-bytes.js";
import { uuid } from "./uuid.js";

/* The attachment block's contract with its host (upload, download link,
 * metadata) and view helpers, without a UI framework: the React view
 * (react/attachment-view.tsx) and the Vue view (vue/AttachmentBlock.vue) share
 * them, and apps/web implements the bridge (features/workspace/attachment-upload.ts). */

export function isStoredAttachmentId(id: string): boolean {
  return uuid.safeParse(id).success;
}

export function decodeFilename(name: string): string {
  if (!/%[0-9A-Fa-f]{2}/.test(name)) return name;
  try {
    return decodeURIComponent(name);
  } catch {
    return name;
  }
}

export interface AttachmentUploadResult {
  id: string;
  name: string;
  image: boolean;
}

export interface AttachmentMeta {
  sizeBytes: number | null;
  mime: string;
  preview: { width: number; height: number } | null;
}

export interface AttachmentBlockBridge {
  upload(
    file: File,
    onProgress: (fraction: number) => void,
    signal?: AbortSignal,
  ): Promise<AttachmentUploadResult>;
  downloadUrl(attachmentId: string): string;
  /** WHY: 치수 예약(C3)·크기/MIME 배지는 GET attachments/:id 메타에서 — 호스트가 없으면 카드는 이름만. */
  attachmentMeta?(attachmentId: string): Promise<AttachmentMeta | null>;
}

/** WHY: 썸네일 잡은 완료 뒤에 돈다 — preview 가 생길 때까지만 1·2·4초 재시도(R5·G2-1). */
export const META_RETRY_MS = [1000, 2000, 4000] as const;

export const ATTACHMENT_ALIGNS = ["left", "center", "right"] as const;
export type AttachmentAlign = (typeof ATTACHMENT_ALIGNS)[number];
export const ATTACHMENT_WIDTH = { min: 25, max: 100, step: 5 } as const;

/** The attachment node's attributes as the views read them. */
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

/** The stored card's size and type badge, e.g. "12 KB · image/png". */
export function attachmentBadge(
  sizeBytes: number | null | undefined,
  mime: string | undefined,
): string {
  return [typeof sizeBytes === "number" ? formatBytes(sizeBytes) : "", mime ?? ""]
    .filter((s) => s.length > 0)
    .join(" · ");
}
