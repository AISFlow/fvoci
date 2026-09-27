# PR132 wiki collab revoke e2e follow-up (task_175cfd8ddd60)

## CI baseline (unchanged claim)

Collaboration-flow job on PR132 at fix `8fe2a941`: **1 failed, 25 passed** (membership revoke scenario). This follow-up does not re-run browser e2e locally (no ZIP slot); pass/fail is not re-verified here.

## What changed in this delta

- Restored eviction wait bound to **20_000 ms** (same order as the prior `[data-collab-status="unauthorized"]` assertion). Raising to 25_000 for consistency with `task-stream-resync.spec.ts` was reverted.
- Kept access-stream eviction assertions: `?denied=workspace`, home `role="alert"` copy containing 접근 권한, zero editor nodes.
- Kept blocked direct return: `memberPage.goto(doc.url)` after revoke.
- Tightened URL matchers to `/\?denied=workspace$/` to match `workspace-flow.spec.ts` / `project-task-flow.spec.ts` for synchronous shell denial.

## Source inference (not experimentally proven here)

| Observation | Inference from code |
| --- | --- |
| CI failed on old assertion | Test waited for in-editor `[data-collab-status="unauthorized"]` and collab-specific alert text. |
| Product path after membership DELETE | `useWorkspaceAccessWatch` refetches workspace list on access-stream signal and `navigate("/?denied=workspace")` when the id is missing (`use-workspace-access-watch.ts`). |
| Direct navigation to document URL | `WorkspaceLayout` resolves slug via `useWorkspaceContext`; if slug ∉ user list, `<Navigate to="/?denied=workspace" />` (`WorkspaceLayout.tsx`). Same query shows `DeniedBanner` with `role="alert"` (`HomePage.tsx`, `error.denied` i18n). |
| Root cause | **Not proven in this worker run** without CI trace/screenshot or local Playwright. Code alignment explains why swapping assertions is plausible; only browser/artifact can confirm timing and flakiness. |

## Verification executed

- `npm run typecheck` in `apps/web` (pass).
- Playwright collab e2e: **not executed** (coordinator ZIP slot).

## Commits

- Follow-up on top of `8fe2a941` (bounded spec + this evidence file).
