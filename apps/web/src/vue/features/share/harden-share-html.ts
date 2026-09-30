import { isSafeShareHref } from "@/lib/share-links";

const ANCHOR_OPEN = /<a\b([^>]*?)>/gi;

function readAttr(attrs: string, name: string): string | null {
  const match = new RegExp(
    `\\b${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`,
    "i",
  ).exec(attrs);
  return match?.[1] ?? match?.[2] ?? match?.[3] ?? null;
}

function stripAttr(attrs: string, name: string): string {
  return attrs.replace(
    new RegExp(`\\s*\\b${name}\\s*=\\s*(?:"[^"]*"|'[^']*'|[^\\s>]+)`, "gi"),
    "",
  );
}

/**
 * React ShareBodyView hardens anchors in useLayoutEffect (before paint).
 * Rewrite the fragment first so v-html never mounts an unsafe href.
 * DOMParser is not in the bun test runtime; the server fragment is already
 * sanitized, so rewriting `<a>` open tags is enough.
 */
export function hardenShareFragmentHtml(html: string): string {
  return html.replace(ANCHOR_OPEN, (_open, attrs: string) => {
    const href = readAttr(attrs, "href");
    let next = stripAttr(stripAttr(attrs, "target"), "rel");
    if (href === null || !isSafeShareHref(href)) {
      next = stripAttr(next, "href");
      return `<a${next}>`;
    }
    return `<a${next} target="_blank" rel="noopener noreferrer">`;
  });
}
