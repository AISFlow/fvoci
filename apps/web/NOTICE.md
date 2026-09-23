UI components adapted from fvoci/FVOCI apps/web at pinned reference
393795261322b916e588043cf94feca999175843:

- src/features/auth/{auth-layout,auth-form,login,setup,wordmark,auth-shell.css}
- src/features/workspace/{workspace-create-dialog,workspace-aux.css,empty-workspace,workspace-shell,wiki-home-view}
  (wiki-home-view from home-view.tsx; workspace-shell is a slice-local shell)
- src/features/documents/{document-view,document-shell.css}
- src/features/settings/settings-shell.css (workspace name/identity section styles)

Unsupported source auth flows (magic link, OIDC, MFA, consent) and workspace
settings (members, import, export, delete) are not wired in this slice.
Document body editing and collaborative editing are not wired in this slice.
