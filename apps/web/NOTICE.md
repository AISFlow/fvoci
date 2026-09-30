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
adaptation. The verified Editor and Calendar adaptations are recorded below.

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


## Official Nuxt UI Editor template adaptation (PR272)

Source repository: https://github.com/nuxt-ui-templates/editor
Source commit: `60886bda1442549b90312ab5097a449eff634fd1`.
FVOCI implementation inspected at `070a859e837b9daa027076a45a5b700465b4ecb0`.
The following are adaptations of presentation and interaction patterns:

| Upstream source path | FVOCI destination | Adapted portion |
| --- | --- | --- |
| `app/composables/useEditorToolbar.ts`, `app/pages/index.vue` | `apps/web/src/vue/features/editor/useEditorToolbar.ts`, `TemplateToolbar.vue` | Grouped history/format/insert items, fixed and selection toolbar composition and link slot; rendered against the existing editor with FVOCI commands, focus, composition and permission guards. |
| `app/components/editor/LinkPopover.vue` | `apps/web/src/vue/features/editor/LinkPopover.vue` | Link input/apply/remove presentation and mark-range command pattern; FVOCI retains its selection, portal, focus, Escape/Tab and IME policy. |
| `app/pages/index.vue` | `packages/editor/src/vue/chrome/editor-template.css`, `apps/web/src/vue/features/editor/editor-controls.css`, `apps/web/src/vue/features/documents/{WikiDocumentView,ProjectDocumentView}.vue`, `apps/web/src/vue/features/tasks/TaskBodyEditor.vue` | Responsive padded, centered editor content and toolbar hosting; package styles are loaded through the existing lazy editor controls. The document views retain their original data/provider lifecycle. |
| `app/composables/useEditorDragHandle.ts`, `app/pages/index.vue` | `packages/editor/src/vue/block-gutter.ts`, `apps/web/src/vue/features/editor/BlockGutter.vue` | Lock the hovered drag handle while its menu owns focus; plus/grip controls. The existing creation-time plugin and FVOCI block commands remain the owners. |
| `app/composables/useEditorSuggestions.ts`, `app/pages/index.vue` | `packages/editor/src/suggestion-menu.ts`, `packages/editor/src/vue/chrome/editor-template.css` | Style/insert menu grouping and compact suggestion presentation shared by existing slash/mention/emoji rendering; FVOCI keeps its item producers, localized titles, keyboard handling, ordering and custom nodes. |

Paths without a full prefix in the first row share
`apps/web/src/vue/features/editor/`. This central source notice also records
those package-level adaptations used by the web editor. The inspected delta
adds no assets or dependencies. Upstream demo mention identities, emoji data,
collaboration provider, AI/upload/backend, body state and schema are not part
of these mappings. Actual mention/entity caller wiring and full Editor
acceptance remain separate work; this notice records implemented adaptations.

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

## Official Nuxt UI Calendar template adaptation (PR272)

Source repository: https://github.com/nuxt-ui-templates/calendar
Source commit: `11809148a32a40612d1d7ddab8aef5372ad46edf`.
FVOCI implementation inspected at `734bc83f4c0786218dd2d0a3455f3950bfca89ac`.
All destinations below are within
`apps/web/src/vue/features/collections/calendar/`:

| Upstream source path | FVOCI destination | Adapted portion |
| --- | --- | --- |
| `app/pages/[view]/[date].vue`, `app/components/AppSidebar.vue`, `app/composables/useCalendar.ts` | `CollectionCalendar.vue` | Date/title controls, day/week/month switching and responsive sidebar composition, with FVOCI collection range/query state. |
| `app/components/calendar/Mini.vue`, `app/components/AppSidebar.vue` | `CalendarMini.vue` | Compact sidebar date selection; implemented as FVOCI's existing month-grid table, using the product week-start/date contract. |
| `app/components/calendar/{MonthView,MonthWeek,WeekView,DayColumn,EventChip}.vue` | `CollectionCalendar.vue`, `calendar.css` | Month cells/chips, week/day time columns, all-day and timed presentation. FVOCI dates/instants remain task values rather than upstream event intervals. |
| `app/components/calendar/{EventPopover,EventForm}.vue` | `CollectionCalendar.vue`, `CalendarEventEditor.vue` | Anchored event popover and date/time edit form; explicit guarded FVOCI submit/reload replaces upstream delayed autosave and unmount flush. |
| `app/composables/useEventMove.ts` | `CollectionCalendar.vue` | Pointer gesture state, cell hit-testing, drag threshold and trailing-click suppression, joined to existing native drag and guarded FVOCI date writes. |

`CollectionContents.vue` hosts this component and retains the existing query,
optimistic rollback, version/expectedDates and Rust-authorized persistence.
`calendar-adapter.ts` is FVOCI product policy, not copied upstream date/store
logic. The inspected delta adds no assets or dependencies; no upstream event
store, demo API, Nuxt payload cache, date-fns implementation or 200ms autosave
is copied. This notice records actual source adaptations while Calendar
regressions, independent review and full UI acceptance remain pending.

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
