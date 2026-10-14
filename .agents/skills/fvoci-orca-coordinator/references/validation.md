# Copy and validation

Copy this whole package, including its tests, to the chosen skill installation directory. Do not copy a locator, Run memory, reports or launch markers with it. Compare the source and installed files before review. The package has no dependency installation step. Its runtime inputs are Bun, Git, the selected Orca CLI and a verified private local profile; the execution adapter additionally needs its supported agent CLI. Resolve the Orca executable using that installation's documented interface, then put its absolute path in the profile. Do not fall back to a different executable or Run after refusal.

Select local tools explicitly. The commands below use these operator-supplied absolute paths:

- `SKILL_DIR`: copied package directory.
- `FVOCI_BUN`: Bun executable, version 1.4.3 for this validation baseline.
- `FVOCI_TSC`: installed TypeScript `bin/tsc`, version 6.0.3.
- `FVOCI_TYPE_ROOTS`: installed `@types` directory containing `bun` and its Node declarations.
- `FVOCI_PRETTIER`: installed Prettier `bin/prettier.cjs`, version 3.9.10.
- `FVOCI_SKILL_TEST_ROOT`: existing owned on-disk fixture directory, outside the copied package.
- `FVOCI_PROFILE`: user-owned mode-0600 locator built from this installation's verified receipts.

These tools can be supplied by an existing repository installation; their paths are configuration, not package contents. Check versions before running commands. A missing executable, declaration package or wrong version is MISSING or a preparation failure, not a passed check. Use the assigned heavy-command wrapper for tests when the surrounding work requires one. Run from an unrelated cwd; none of these commands needs the original checkout or its scratch.

```sh
"$FVOCI_BUN" --version
"$FVOCI_BUN" "$FVOCI_TSC" --version
"$FVOCI_BUN" "$FVOCI_PRETTIER" --version
export PATH="$(dirname "$FVOCI_BUN"):$PATH"
export FVOCI_SKILL_TEST_ROOT
"$FVOCI_BUN" run --cwd "$SKILL_DIR" test
"$FVOCI_BUN" "$FVOCI_TSC" --noEmit --strict --target ESNext --module Preserve --moduleResolution bundler --types bun --typeRoots "$FVOCI_TYPE_ROOTS" --skipLibCheck "$SKILL_DIR/scripts/coordinator.ts" "$SKILL_DIR/scripts/legacy.ts" "$SKILL_DIR/scripts/coordinator.test.ts"
"$FVOCI_BUN" "$FVOCI_PRETTIER" --check --no-config --no-editorconfig --print-width 100 --html-whitespace-sensitivity strict "$SKILL_DIR"
```

`skipLibCheck` excludes external declarations from rechecking; package sources and tests still receive strict checking. Formatting uses explicit package settings and otherwise Prettier defaults, so an ancestor checkout's configuration cannot affect the copy. Formatting Markdown and YAML does not validate skill-host metadata. If the destination host supplies a metadata validator, run its documented command against `SKILL_DIR` and record that result separately; the package requires no particular vendor's validator. Report an unavailable requested validator as MISSING. Preserve the existing UI metadata and invocation policy in its separate host metadata file.

Check local reference targets separately from formatting and metadata. The links in this package resolve to `references/operations.md`, `references/migration.md`, `references/validation.md`, `scripts/coordinator.ts` and `scripts/legacy.ts`. Open each from the copied Markdown's own directory and confirm that the target is a regular file inside the copy. Do not treat external FVOCI policy files as bundled references, invent a Markdown/YAML parser, or use a private scratch validator as a required check.

The test command uses generated local CLI fixtures and real Bun children. It checks exact source mapping and lineage, clean fixed source, controller/worker banners, requests and all three compatibility argv interfaces, complete receipts, one waiter, full delivery decisions, ack-returned mail and partial-result preservation. It also invokes the copied `inspect --profile` command from a foreign cwd with an explicit local fixture profile and no inherited locator or terminal binding, checking success and a foreign-Run refusal. It does not launch live agents or consume live mail.

For stronger relocation proof, run these commands in a disposable filesystem view or container that mounts only the copied package, the explicitly selected tool installation and fresh writable fixtures. Use a different installation path and cwd inside it. Leave the original checkout, original scratch and user state unmounted, clear inherited environment values, and supply only the fixture path, selected tool PATH and on-disk TMPDIR. Record the actual mounts/environment and test count; changing cwd alone does not prove that the original paths were inaccessible. This isolation is a validation setup, not a new runtime prerequisite. Keep failed preparations and their logs outside the installed package.

Finally, perform only the read-only local inspection when authorized:

```sh
"$FVOCI_BUN" "$SKILL_DIR/scripts/coordinator.ts" inspect --profile "$FVOCI_PROFILE"
```

A successful result must identify the selected Run/coordinator, parent and external memory references. A refusal is a supported safe outcome when local ownership cannot be verified: inspect the selected CLI's read-only receipts and have the current owner correct or supply the profile or complete takeover separately. Never substitute another Run/repository/tool, create a replacement locator from guesses, or weaken a banner/lineage check to make inspection pass. Local fixture success and metadata validity do not establish live launch, takeover, wait, settlement or cleanup support; record those as NOTRUN until separately authorized.
