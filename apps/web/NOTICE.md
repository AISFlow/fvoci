UI components adapted from fvoci/FVOCI apps/web at pinned reference
393795261322b916e588043cf94feca999175843:

- src/features/auth/{auth-layout,auth-form,login,setup,wordmark,auth-shell.css}
- src/features/workspace/{workspace-create-dialog,workspace-aux.css,empty-workspace,workspace-shell,wiki-home-view}
  (wiki-home-view from home-view.tsx; workspace-shell is a slice-local shell)
- src/features/documents/{document-view,document-shell.css}
- src/features/settings/settings-shell.css (workspace name/identity section styles)

Unsupported source auth flows (magic link, OIDC, MFA, consent) are not wired
in this slice. Workspace settings members UI is not copied; the member
role/removal HTTP API is used by collab revoke E2E once /collab is live.
Document body uses FvociEditor + collab-session against cookie /collab.
Attachment upload, mention search, unfurl, comments, AI, tags, collections,
revisions, share, and md/PDF export stay unavailable without fake success.

Additionally adapted from the same source SHA:

- packages/editor (schema, FvociEditor, collab constants; no export/office)
- apps/web/src/features/documents/{collab-session,block-presence}
- packages/ui/presence.ts → src/lib/presence.ts

