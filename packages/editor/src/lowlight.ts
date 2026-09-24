import { createLowlight } from "lowlight";

export const HIGHLIGHT_MAX_CHARS = 100_000;

const inner = createLowlight();

function uncolored(value: string): ReturnType<typeof inner.highlight> {
	return { type: "root", children: [{ type: "text", value }] };
}

export const lowlight = {
	highlight(
		language: string,
		value: string,
		options?: Parameters<typeof inner.highlight>[2],
	) {
		if (value.length > HIGHLIGHT_MAX_CHARS) return uncolored(value);
		return inner.highlight(language, value, options);
	},
	highlightAuto(
		value: string,
		options?: Parameters<typeof inner.highlightAuto>[1],
	) {
		if (value.length > HIGHLIGHT_MAX_CHARS) return uncolored(value);
		return inner.highlightAuto(value, options);
	},
	listLanguages: () => inner.listLanguages(),
	register: inner.register.bind(inner),
	registered: (name: string) => inner.registered(name),
};

const LOADERS: Record<string, () => Promise<unknown>> = {
	typescript: () => import("highlight.js/lib/languages/typescript"),
	javascript: () => import("highlight.js/lib/languages/javascript"),
	json: () => import("highlight.js/lib/languages/json"),
	css: () => import("highlight.js/lib/languages/css"),
	python: () => import("highlight.js/lib/languages/python"),
	diff: () => import("highlight.js/lib/languages/diff"),
};

const ALIAS: Record<string, string> = {
	ts: "typescript",
	js: "javascript",
	py: "python",
	patch: "diff",
};

export type FenceLanguage = {
	language: string;
	highlightLines: number[];
};

export function parseHighlightLines(spec: string | undefined): number[] {
	if (!spec) return [];
	const out: number[] = [];
	const seen = new Set<number>();
	for (const part of spec.split(",")) {
		const range = part.trim();
		if (!range) continue;
		const dash = range.indexOf("-");
		const start = Number(dash === -1 ? range : range.slice(0, dash));
		const end = Number(dash === -1 ? range : range.slice(dash + 1));
		if (!Number.isInteger(start) || !Number.isInteger(end)) continue;
		const lo = Math.min(start, end);
		const hi = Math.max(start, end);
		if (lo < 1) continue;
		for (let n = lo; n <= hi; n++) {
			if (seen.has(n)) continue;
			seen.add(n);
			out.push(n);
		}
	}
	return out;
}

/* WHY: ALIAS 를 그냥 인덱싱하면 ```constructor 같은 이름이 프로토타입 체인에서 함수를
 * 물어 온다 — 선언은 string 인데 함수가 담기고, 협업 문서 쓰기가 그 자리에서 깨진다. */
function aliasOf(name: string): string {
	return Object.hasOwn(ALIAS, name) ? (ALIAS[name] ?? name) : name;
}

export function parseFenceLanguage(raw: string | undefined): FenceLanguage {
	if (!raw) return { language: "", highlightLines: [] };
	const trimmed = raw.trim();
	const m = /^([^{}]*?)(?:\{([^}]*)\})?$/.exec(trimmed);
	if (!m) return { language: aliasOf(trimmed), highlightLines: [] };
	const langRaw = (m[1] ?? "").trim();
	return {
		language: aliasOf(langRaw),
		highlightLines: parseHighlightLines(m[2]),
	};
}

export function languageOfFence(raw: string | undefined): string {
	return parseFenceLanguage(raw).language;
}

export async function ensureLanguage(
	lang: string,
	source = "",
): Promise<string> {
	const key = languageOfFence(lang) || lang;
	if (source.length > HIGHLIGHT_MAX_CHARS) return key;
	if (lowlight.listLanguages().includes(key)) return key;
	const load = LOADERS[key];
	if (!load) return key;
	const mod = await load();
	const fn =
		typeof mod === "object" && mod !== null && "default" in mod
			? mod.default
			: mod;
	if (typeof fn === "function") {
		lowlight.register(key, fn as Parameters<typeof lowlight.register>[1]);
	}
	return key;
}
