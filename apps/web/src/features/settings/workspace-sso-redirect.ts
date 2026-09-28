// The redirect URI a workspace's SSO provider must register. Each workspace
// completes sign-in at its own callback (server
// `OidcSettings::workspace_redirect_uri`), so the admin copies it into the
// identity provider's client before saving the issuer and credentials here.
import { createElement, useId } from "react";
import { t } from "@fvoci/i18n";

export type RedirectCopyStatus = "copied" | "failed" | null;

/** `{origin}/api/v1/auth/sso/{workspaceId}/callback`, as the server builds it. */
export function workspaceSsoRedirectUri(origin: string, workspaceId: string): string {
  const base = origin.replace(/\/+$/, "");
  return `${base}/api/v1/auth/sso/${encodeURIComponent(workspaceId)}/callback`;
}

export async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

export function WorkspaceSsoRedirectUri({
  uri,
  copyStatus,
  onCopy,
}: {
  uri: string;
  copyStatus: RedirectCopyStatus;
  onCopy: () => void;
}) {
  const id = useId();
  const inputId = `${id}-redirect-uri`;
  const helpId = `${id}-redirect-uri-help`;
  return createElement(
    "div",
    { className: "flex flex-col gap-1.5", "data-testid": "workspace-sso-redirect-uri" },
    createElement(
      "label",
      { htmlFor: inputId, className: "text-ui font-medium leading-none select-none" },
      t("auth.sso.redirectUri"),
    ),
    createElement(
      "p",
      { id: helpId, className: "text-ui text-muted-foreground" },
      t("auth.sso.redirectUri.help"),
    ),
    createElement(
      "div",
      { className: "flex flex-wrap items-center gap-2" },
      createElement("input", {
        id: inputId,
        readOnly: true,
        value: uri,
        autoComplete: "off",
        spellCheck: false,
        "aria-describedby": helpId,
        className:
          "h-11 w-full min-w-0 flex-1 rounded-md border border-input bg-background px-3 py-2 font-mono text-ui outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring",
        onFocus: (event: { currentTarget: HTMLInputElement }) => event.currentTarget.select(),
      }),
      createElement(
        "button",
        {
          type: "button",
          className:
            "inline-flex h-8 items-center justify-center rounded-md border border-border bg-background px-3 text-sm font-medium transition-colors hover:bg-accent hover:text-foreground",
          onClick: onCopy,
        },
        t(copyStatus === "copied" ? "auth.sso.redirectUri.copied" : "auth.sso.redirectUri.copy"),
      ),
    ),
    copyStatus === "failed"
      ? createElement(
          "p",
          { role: "alert", className: "settings-notice settings-notice--danger" },
          t("auth.sso.redirectUri.copyFailed"),
        )
      : null,
  );
}
