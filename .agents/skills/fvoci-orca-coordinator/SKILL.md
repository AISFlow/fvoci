---
name: fvoci-orca-coordinator
description: "Resume or coordinate FVOCI work supervised in Orca across sessions or work locations, including verified child lineage, worker launch profiles, and delivery handling. Use for FVOCI controller handover or supervised worker operations; ordinary product coding does not need this skill."
---

# FVOCI Orca coordination

Preserve the existing coordination chain across sessions. This skill connects FVOCI's external handoff memory and launch profile to the installed Orca contract; it grants no lifecycle or repository authority.

Resolve the user locator at `${XDG_STATE_HOME:-$HOME/.local/state}/fvoci-orca/current.json`, or an explicitly supplied `--profile` file. It must be a user-owned regular file with mode 0600. Never search unrelated runs to replace a missing locator. Run `bun <this-skill>/scripts/coordinator.ts inspect` from any cwd, then read the locator's `handoff` and recent `state` entries. These documents are the work-memory authority; an old `run.json` is historical evidence. No current task, terminal, SHA, session, or PR belongs in this skill.

Load `skills get orca-cli` and `skills get orchestration` through the locator's executable, then the conditional references named by those guides. The helper checks runtime, exact Run/coordinator, source-to-registered-repository mapping, parent identity, controller banner and terminal ownership without binding a Run, consuming mail, or changing live resources. Historical HANDOFF commands are not current authority: compare them with current state and actual Orca receipts before any takeover. Missing paths, unavailable CLI, stale coordinator, conflicting parent, or unknown receipt require a report to the current owner. Do not create a replacement Run, worker, waiter, or cleanup action by inference.

Read the target checkout's AGENTS.md and selected FVOCI skills for product, verification, submission and approval rules. Do not copy those policies into this package. Read [operations.md](references/operations.md) when launching or handling mail; it describes the helper's request and decision inputs. Read [migration.md](references/migration.md) before replacing an existing launcher.

Use the external profile for controller and worker model/effort/tier, concurrency guidance, common prompts and heavy-command entrypoint. Verify actual TUI model, effort and Fast state; a successful launch flag alone is not proof. Preserve agents from other sessions, their worktrees, dirty files, processes and DBs. Narrow an operation to its explicit task and ownership; never restart or stop another chain to satisfy this one.

For review and acceptance, keep fixed source identity, author/reviewer independence, exact checks and their counts/exit codes, and PASS/FAIL/NOTRUN/MISSING separate. Read the complete report before accepting its summary. A heartbeat is liveness evidence, not completion; CLI transport keepalive, PTY activity, fleet agent liveness and Task settlement are distinct. Use Orca's current receipts and guides for settlement, retention, release and recovery. This package never performs release, stop, retry, cleanup, Git push or CI mutation.

Only one owner waits on a Run. Process every message in the returned batch, answer questions by original message ID, and finish the required ownership decision before acknowledging it. Inspect the acknowledgment's returned batch too. The helper does not decide acceptance or termination and never runs an autonomous scheduler. If a waiter already exists, leave it intact. New launch tooling remains a candidate until the controller finishes independent review and explicitly switches the existing entrypoints.
