import { asSafeHtml, SafeHtmlView } from "@fvoci/editor/safe-html";
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { unfurlQueryOptions } from "@/lib/queries/workspace";
import {
  isHttpUrl,
  isSandboxedIframeHtml,
  unfurlCardDataOf,
  unfurlDisplayTitle,
  type UnfurlOutput,
} from "./unfurl";

export type UnfurlCardState =
  | { state: "loading" }
  | { state: "failed"; url: string }
  | {
      state: "resolved";
      url: string;
      title: string;
      description: string;
      imageUrl: string | null;
    };

function OpenLink({ url }: { url: string }) {
  if (!isHttpUrl(url)) return null;
  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      className="inline-flex h-8 items-center rounded-md border border-border bg-background px-3 text-sm font-medium hover:bg-accent"
    >
      {t("unfurl.open")}
    </a>
  );
}

export function UnfurlCardView({ state }: { state: UnfurlCardState }) {
  if (state.state === "loading") {
    return (
      <article className="w-full max-w-md rounded-lg border border-border py-4" data-entity="url">
        <p className="px-4 text-doc" role="status">
          {t("unfurl.loading")}
        </p>
      </article>
    );
  }
  if (state.state === "failed") {
    return (
      <article className="w-full max-w-md rounded-lg border border-border py-4" data-entity="url">
        <p className="break-keep px-4 text-doc">{t("unfurl.failed")}</p>
        <div className="px-4 pt-2">
          <OpenLink url={state.url} />
        </div>
      </article>
    );
  }
  const title = unfurlDisplayTitle(state.title, state.url);
  return (
    <article
      className="w-full max-w-md overflow-hidden rounded-lg border border-border"
      data-entity="url"
    >
      {state.imageUrl && isHttpUrl(state.imageUrl) ? (
        <img src={state.imageUrl} alt="" className="aspect-video w-full object-cover" />
      ) : null}
      <div className="px-4 pt-4">
        <h2 className="break-keep text-doc font-medium">{title}</h2>
        {state.description !== "" ? (
          <p className="break-keep pt-1 text-ui text-muted-foreground">{state.description}</p>
        ) : null}
      </div>
      <div className="px-4 pt-2 pb-4">
        <OpenLink url={state.url} />
      </div>
    </article>
  );
}

function parseUnfurl(data: unknown): UnfurlOutput | null {
  if (data === null || typeof data !== "object") return null;
  const value = data as Partial<UnfurlOutput>;
  if (value.kind !== "github_issue" && value.kind !== "github_pull" && value.kind !== "og") {
    return null;
  }
  if (typeof value.url !== "string") return null;
  return value as UnfurlOutput;
}

export function UnfurlCard({
  workspaceId,
  url,
}: {
  workspaceId: string | null;
  url: string;
}) {
  const allowed = isHttpUrl(url);
  const query = useQuery({
    ...unfurlQueryOptions(workspaceId, url),
    enabled: workspaceId !== null && allowed,
  });
  if (!allowed) {
    return <UnfurlCardView state={{ state: "failed", url }} />;
  }
  if (query.isLoading) {
    return <UnfurlCardView state={{ state: "loading" }} />;
  }
  if (query.isError) {
    return <UnfurlCardView state={{ state: "failed", url }} />;
  }
  const parsed = parseUnfurl(query.data);
  if (!parsed) {
    return <UnfurlCardView state={{ state: "failed", url }} />;
  }
  const data = unfurlCardDataOf(parsed);
  if (isSandboxedIframeHtml(parsed.html)) {
    return <SafeHtmlView className="fvoci-oembed" html={asSafeHtml(parsed.html)} />;
  }
  return (
    <UnfurlCardView
      state={{
        state: "resolved",
        url,
        title: data.title,
        description: data.description,
        imageUrl: data.imageUrl,
      }}
    />
  );
}
