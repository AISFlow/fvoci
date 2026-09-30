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


## Official Nuxt UI Dashboard template adaptation (PR272)

The PR272 candidate inspected at FVOCI commit
`b00bbd11fe59dbfb15604b8c036c5c238eea9144` adapts the following UI patterns:

- Destination: `apps/web/src/vue/components/WorkspaceShell.vue`.
- Source repository: https://github.com/nuxt-ui-templates/dashboard
- Source commit: `57e8a76e85ac382f2dd75946aa450afb1b3e4b0d`.
- Source paths: `app/layouts/default.vue` (DashboardGroup/navigation-menu
  composition) and `app/pages/index.vue` (DashboardPanel/Navbar header/body slots).
- Adaptation: FVOCI workspace navigation items and panel/header/body/footer
  composition using Nuxt UI components, with FVOCI routing, session, queries,
  search, notification, logout and legal controls retained.

This entry covers that verified component adaptation. It does not attribute
other proposed template mappings as copied code. No template assets, demo data,
server/auth/storage layers, manifests or dependencies were copied in that
adaptation. Editor and Calendar source notices must be added if their code is
actually copied or adapted in a later candidate.

Upstream `LICENSE` (SHA-256
`e40f408c466e72a3b02eabe846ef35cfab210c14daff782a072dd4903d911f93`):

MIT License

Copyright (c) 2025 Nuxt UI Templates

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
