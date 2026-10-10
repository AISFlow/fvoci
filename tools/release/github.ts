// GitHub release state for one tag, from the releases list (drafts have no
// tag lookup; they are listed for a token with contents: write).
import { Fail, parseJson } from "./fail.ts";
import { HttpStatus, send, type HttpClient } from "./http.ts";

export const REPOSITORY = /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/;
const PER_PAGE = 100;
const MAX_PAGES = 50;

export interface ReleaseState {
  state: "none" | "published" | "draft";
  assets: string[];
}

export async function releaseState(
  http: HttpClient,
  api: string,
  repository: string,
  token: string,
  tag: string,
): Promise<ReleaseState> {
  const headers = {
    Authorization: `Bearer ${token}`,
    Accept: "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  const base = api.replace(/\/+$/, "");
  const matching: Record<string, unknown>[] = [];
  let complete = false;
  for (let page = 1; page <= MAX_PAGES; page += 1) {
    let body: Uint8Array;
    try {
      const url = `${base}/repos/${repository}/releases?per_page=${String(PER_PAGE)}&page=${String(page)}`;
      body = (await send(http, { url, method: "GET", headers })).body;
    } catch (error) {
      if (error instanceof HttpStatus)
        throw new Fail(`listing releases of ${repository}: HTTP ${String(error.response.status)}`);
      throw error;
    }
    const batch = parseJson(body);
    if (!Array.isArray(batch))
      throw new Fail(`listing releases of ${repository}: page ${String(page)} is not a list`);
    for (const release of batch as unknown[]) {
      if (typeof release !== "object" || release === null || Array.isArray(release))
        throw new Fail(`listing releases of ${repository}: bad entry`);
      if ((release as Record<string, unknown>).tag_name === tag)
        matching.push(release as Record<string, unknown>);
    }
    if (batch.length < PER_PAGE) {
      complete = true;
      break;
    }
  }
  if (!complete)
    throw new Fail(`more than ${String(PER_PAGE * MAX_PAGES)} releases; refusing to guess`);
  if (matching.length > 1)
    throw new Fail(
      `${String(matching.length)} releases name ${tag}; delete the extra ones by hand`,
    );
  const [release] = matching;
  if (release === undefined) return { state: "none", assets: [] };
  const assets = release.assets === undefined ? [] : release.assets;
  if (!Array.isArray(assets)) throw new Fail(`release ${tag}: assets is not a list`);
  const names = assets.map((asset: unknown) => {
    const name =
      typeof asset === "object" && asset !== null && !Array.isArray(asset)
        ? (asset as Record<string, unknown>).name
        : undefined;
    if (typeof name !== "string") throw new Fail(`release ${tag}: asset without a name`);
    return name;
  });
  // GitHub sends a boolean; anything else is an answer we cannot classify.
  const draft = release.draft ?? false;
  if (typeof draft !== "boolean") throw new Fail(`release ${tag}: draft is not a boolean`);
  return { state: draft ? "draft" : "published", assets: names.sort(compareCodePoints) };
}

function compareCodePoints(a: string, b: string): number {
  const left = Array.from(a);
  const right = Array.from(b);
  for (let i = 0; i < Math.min(left.length, right.length); i += 1) {
    const delta = (left[i]?.codePointAt(0) ?? 0) - (right[i]?.codePointAt(0) ?? 0);
    if (delta !== 0) return delta;
  }
  return left.length - right.length;
}

/** `json.dumps` default layout (", " / ": ", ASCII only), as the shell callers have always read it. */
export function formatReleaseState(state: ReleaseState): string {
  const quote = (value: string) =>
    JSON.stringify(value).replace(
      // JSON.stringify already escapes control characters; Python also escapes DEL and non-ASCII.
      /[\u007f-\uffff]/g,
      (ch) => `\\u${ch.charCodeAt(0).toString(16).padStart(4, "0")}`,
    );
  return `{"state": ${quote(state.state)}, "assets": [${state.assets.map(quote).join(", ")}]}`;
}
