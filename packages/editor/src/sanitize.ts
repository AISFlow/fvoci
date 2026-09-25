/* WHY: this repository has no @fvoci/contracts package; values copied from
 * source packages/contracts (routes.ts API_PREFIX, primitives.ts UUID_SOURCE). */
const API_PREFIX = "/api/v1";
const UUID_SOURCE = "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
import sanitizeHtml from "sanitize-html";

export const NON_TEXT_TAGS = ["script", "style", "textarea", "option"] as const;

const BLOCKNOTE_TAGS = [
	"div",
	"p",
	"h1",
	"h2",
	"h3",
	"ul",
	"ol",
	"li",
	"blockquote",
	"pre",
	"code",
	"table",
	"thead",
	"tbody",
	"tr",
	"th",
	"td",
	"img",
	"a",
	"span",
	"strong",
	"em",
	"s",
	"u",
	"mark",
	"details",
	"summary",
	"hr",
	"br",
	"input",
	"label",
	"figure",
	"figcaption",
] as const;

const KATEX_TAGS = [
	"math",
	"semantics",
	"annotation",
	"mrow",
	"mi",
	"mo",
	"mn",
	"ms",
	"mtext",
	"mspace",
	"msup",
	"msub",
	"msubsup",
	"mfrac",
	"msqrt",
	"mroot",
	"mstyle",
	"merror",
	"mpadded",
	"mphantom",
	"munder",
	"mover",
	"munderover",
	"mtable",
	"mtr",
	"mtd",
	"mmultiscripts",
	"mprescripts",
] as const;

const NUMERIC_UNIT = [/^-?[\d.]+(?:em|ex|px|rem|pt|%)$/];
const ALLOWED_STYLES: NonNullable<sanitizeHtml.IOptions["allowedStyles"]> = {
	"*": {
		height: NUMERIC_UNIT,
		width: NUMERIC_UNIT,
		"min-width": NUMERIC_UNIT,
		top: NUMERIC_UNIT,
		left: NUMERIC_UNIT,
		"margin-left": NUMERIC_UNIT,
		"margin-right": NUMERIC_UNIT,
		"padding-left": NUMERIC_UNIT,
		"vertical-align": NUMERIC_UNIT,
		"border-bottom-width": NUMERIC_UNIT,
		position: [/^(?:relative|absolute)$/],
	},
};

/* WHY: preview img src 경로 문자셋 — API id 판정이 아니라 URL 문법이라 소문자 hex 를 박는다. */
export const ATTACHMENT_PREVIEW_SRC = new RegExp(
	`^${API_PREFIX}/(?:workspaces/${UUID_SOURCE}|share/[A-Za-z0-9_-]+)/attachments/${UUID_SOURCE}/download\\?variant=preview$`,
);

function isAllowedImgSrc(src: string | undefined): boolean {
	if (src === undefined) return false;
	return ATTACHMENT_PREVIEW_SRC.test(src) || /^https?:\/\//.test(src);
}

export const SANITIZE_OPTIONS: sanitizeHtml.IOptions = {
	allowedTags: [...BLOCKNOTE_TAGS, ...KATEX_TAGS],
	disallowedTagsMode: "discard",
	exclusiveFilter: (frame) =>
		frame.tag === "img" && !isAllowedImgSrc(frame.attribs.src),
	allowedAttributes: {
		"*": [
			"class",
			"aria-hidden",
			"data-node-type",
			"data-id",
			"data-content-type",
			"data-kind",
			"data-entity",
			"data-checked",
			"data-level",
			"data-language",
			"data-math",
			"data-math-inline",
			"data-mermaid",
			"data-image",
		],
		a: ["href"],
		figure: ["style", "data-align"],
		img: ["src", "alt", "width", "height"],
		input: ["type", "checked", "disabled"],
		td: ["colspan", "rowspan"],
		th: ["colspan", "rowspan"],
		ol: ["start"],
		span: ["style"],
		math: ["xmlns", "display"],
		annotation: ["encoding"],
	},
	allowedStyles: ALLOWED_STYLES,
	allowedSchemes: ["http", "https", "mailto"],
	allowedSchemesAppliedToAttributes: ["href", "src"],
	allowProtocolRelative: false,
};

export function sanitizeRenderedHtml(html: string): string {
	return sanitizeHtml(html, SANITIZE_OPTIONS);
}
