/*
 * WHY: #602 사람용 주소 — 내부 링크는 `/w/:slug/:ref` 하나다. 워크스페이스는 UUID 가 아니라
 * 슬러그로 식별되므로 대소문자 무시로 비교한다(캐노니컬은 소문자).
 */
import { formatDisplayId, parseDisplayId } from "../display-id.js";
import { uuid } from "../uuid.js";
import type { EntityResolver } from "./blocks.js";

const PATH = /^\/w\/([^/]+)\/([^/]+)/i;

const PASTE_KINDS = ["document", "task", "project"] as const;

type PastedEmbed = {
	entity: (typeof PASTE_KINDS)[number];
	ref: string;
};

/*
 * WHY: `ref` 가 프로젝트 키인지 예약 화면인지는 이 패키지가 알 수 없고 알 필요도 없다 —
 * 항목 참조(표시 ID·UUID)만 임베드로 접고 나머지는 맨 링크로 둔다.
 * WHY: 주소만으로는 문서·태스크가 갈리지 않는다. 기본은 `document` 이고, 종류는
 * EntityResolver 가 같은 해석 결과로 고른다(#491 C).
 */
export function parseWorkspaceUrl(
	raw: string,
	workspaceSlug: string,
): PastedEmbed | null {
	let url: URL;
	try {
		url = new URL(raw.trim());
	} catch {
		return null;
	}
	if (url.protocol !== "http:" && url.protocol !== "https:") return null;
	const m = PATH.exec(url.pathname);
	const slug = m?.[1];
	const ref = m?.[2];
	if (!slug || !ref) return null;
	if (slug.toLowerCase() !== workspaceSlug.toLowerCase()) return null;
	if (uuid.safeParse(ref.toLowerCase()).success) {
		return { entity: "document", ref: ref.toLowerCase() };
	}
	const parsed = parseDisplayId(ref);
	if (parsed) {
		return {
			entity: "document",
			ref: formatDisplayId(parsed.prefix, parsed.n),
		};
	}
	return null;
}

export async function resolvePastedEmbed(
	raw: string,
	workspaceSlug: string,
	resolve: EntityResolver | null,
): Promise<PastedEmbed | null> {
	const parsed = parseWorkspaceUrl(raw, workspaceSlug);
	if (!parsed) return null;
	if (!resolve) return parsed;
	const hits = await Promise.all(
		PASTE_KINDS.map(async (entity) => {
			const snap = await resolve(entity, parsed.ref);
			return snap ? entity : null;
		}),
	);
	const entity = hits.find((kind) => kind !== null) ?? parsed.entity;
	return { entity, ref: parsed.ref };
}
