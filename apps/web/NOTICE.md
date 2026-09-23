UI components adapted from fvoci/FVOCI apps/web at pinned reference
393795261322b916e588043cf94feca999175843:

- src/features/auth/{auth-layout,auth-form,login,setup,wordmark,auth-shell.css}
- src/features/workspace/{workspace-create-dialog,workspace-aux.css,empty-workspace}
- src/features/settings/settings-shell.css (workspace name/identity section styles)

Unsupported source auth flows (magic link, OIDC, MFA, consent) and workspace
settings (members, import, export, delete) are not wired in this slice.
