import { UrlEmbedContext } from "@fvoci/editor/react";
import { type ReactNode, useCallback } from "react";
import { UnfurlCard } from "./unfurl-card";

/** Source `document-editor.tsx` UrlEmbedContext wiring for FvociEditor. */
export function UrlEmbedProvider({
  workspaceId,
  children,
}: {
  workspaceId: string | null;
  children: ReactNode;
}) {
  const renderUrl = useCallback(
    (url: string) => <UnfurlCard workspaceId={workspaceId} url={url} />,
    [workspaceId],
  );
  return (
    <UrlEmbedContext.Provider value={workspaceId ? renderUrl : null}>
      {children}
    </UrlEmbedContext.Provider>
  );
}
