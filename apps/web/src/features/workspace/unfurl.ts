export type UnfurlOutput = {
  kind: "github_issue" | "github_pull" | "og";
  url: string;
  title?: string | null;
  description?: string | null;
  imageUrl?: string | null;
  state?: string | null;
  number?: number | null;
  owner?: string | null;
  repo?: string | null;
  html?: string | null;
};

export interface UnfurlCardData {
  title: string;
  description: string;
  imageUrl: string | null;
}

export function isHttpUrl(value: string): boolean {
  try {
    const parsed = new URL(value);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

function githubTitle(value: UnfurlOutput): string {
  if (value.owner && value.repo && value.number != null) {
    return `${value.owner}/${value.repo}#${String(value.number)}`;
  }
  return "";
}

export function unfurlCardDataOf(value: UnfurlOutput): UnfurlCardData {
  const title = (value.title ?? "").trim() || githubTitle(value);
  const description = (value.description ?? "").trim();
  const imageRaw = (value.imageUrl ?? "").trim();
  const imageUrl = imageRaw !== "" && isHttpUrl(imageRaw) ? imageRaw : null;
  return { title, description, imageUrl };
}

export function unfurlDisplayTitle(title: string, url: string): string {
  if (title !== "") return title;
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

export function isSandboxedIframeHtml(html: string | null | undefined): html is string {
  return Boolean(html?.startsWith("<iframe ") && html.includes("sandbox="));
}
