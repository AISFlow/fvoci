import { afterEach, describe, expect, test } from "bun:test";
import { rmSync } from "node:fs";
import { join } from "node:path";
import { resolveSelectionInputs } from "./events.ts";
import { diffPathsForPr, ensureCommitShas, parseNameStatusZ, repoGit, type Git } from "./git.ts";
import { decideFromPaths } from "./paths.ts";
import type { PyValue } from "./pyjson.ts";
import { git, PrCheckout, removeScratch, Repo, SHA_A, SHA_B, writeFile } from "./test-support.ts";

afterEach(removeScratch);

const TIMEOUT = 60_000;
const bytes = (text: string) => new TextEncoder().encode(text);

/** resolveSelectionInputs for a pull_request event in a real checkout. */
const resolve = (
  work: string,
  base: unknown,
  head: unknown,
  tested: string,
  override: Partial<Git> = {},
) =>
  resolveSelectionInputs(
    { ...repoGit(work), ...override },
    new Map([
      [
        "pull_request",
        new Map<string, PyValue>([
          ["draft", false],
          ["base", new Map([["sha", base as string]])],
          ["head", new Map([["sha", head as string]])],
        ]),
      ],
    ]),
    "pull_request",
    tested,
  );

describe("parseNameStatusZ", () => {
  test("rename, delete, copy and type changes", () => {
    const raw = bytes(
      "A\0src/new.ts\0D\0src/old.ts\0R100\0old-name\0new-name\0C075\0a\0b\0T\0link\0",
    );
    expect(parseNameStatusZ(raw)).toEqual({
      value: ["src/new.ts", "src/old.ts", "old-name", "new-name", "a", "b", "link"],
      error: null,
    });
    expect(parseNameStatusZ(new Uint8Array())).toEqual({ value: [], error: null });
  });

  test("truncated and malformed records fail", () => {
    const cases: [string, string][] = [
      ["R100\0only-old\0", "DIFF_TRUNCATED_RENAME"],
      ["A\0file.ts", "DIFF_TRUNCATED"],
      ["M\0AGENTS.md\0M\0.agents/environment.md", "DIFF_TRUNCATED"],
      ["D\0", "DIFF_TRUNCATED_DELETE"],
      ["M\0", "DIFF_TRUNCATED_PATH"],
      ["\0a\0", "DIFF_EMPTY_STATUS"],
      ["X\0a\0", "DIFF_BAD_STATUS"],
      ["M1x\0a\0", "DIFF_BAD_STATUS"],
    ];
    for (const [raw, code] of cases)
      expect(parseNameStatusZ(bytes(raw)), raw).toEqual({ value: null, error: code });
  });

  test("a path that is not UTF-8 is refused", () => {
    expect(() => parseNameStatusZ(new Uint8Array([0x4d, 0, 0xff, 0]))).toThrow();
  });

  test("a leading U+FEFF is part of the path", () => {
    expect(parseNameStatusZ(bytes("A\0﻿apps/web/src/x.vue\0R100\0﻿a\0﻿b\0"))).toEqual({
      value: ["﻿apps/web/src/x.vue", "﻿a", "﻿b"],
      error: null,
    });
  });
});

describe("U+FEFF file names through real git", () => {
  const BOM_VUE = "﻿apps/web/src/x.vue";
  test(
    "an added BOM-prefixed path stays unknown and plans full",
    () => {
      const fx = new PrCheckout();
      const head = fx.branch({ [BOM_VUE]: "<template />\n" });
      const m = fx.merge(fx.base, head);
      const inputs = resolve(m.work, fx.base, head, m.tested);
      expect(inputs.paths).toEqual([BOM_VUE]);
      expect(decideFromPaths(inputs.paths ?? []).reasonCode).toBe("FULL_UNKNOWN_PATH");
    },
    TIMEOUT,
  );

  test(
    "a rename to or from a BOM-prefixed path keeps both names and plans full",
    () => {
      const body = "<template>same</template>\n".repeat(20);
      for (const [from, to] of [
        ["apps/web/src/x.vue", BOM_VUE],
        [BOM_VUE, "apps/web/src/y.vue"],
      ] as const) {
        const fx = new PrCheckout();
        const base = fx.advance({ [from]: body });
        git(fx.origin.dir, "checkout", "-q", "-B", "rename", "main");
        const head = fx.origin.rename(from, to);
        git(fx.origin.dir, "checkout", "-q", "main");
        const m = fx.merge(base, head);
        const inputs = resolve(m.work, base, head, m.tested);
        expect(inputs.paths?.sort()).toEqual([from, to].sort());
        expect(decideFromPaths(inputs.paths ?? []).reasonCode, `${from} -> ${to}`).toBe(
          "FULL_UNKNOWN_PATH",
        );
      }
    },
    TIMEOUT,
  );
});

describe("git reads", () => {
  test("multi-commit diff from the merge base", () => {
    const repo = new Repo();
    const base = repo.commit({ "docs/rewrite.md": "a\n" });
    const head = repo.commit({ "docs/rewrite.md": "b\n" });
    const pr = diffPathsForPr(repoGit(repo.dir), base, head);
    expect(pr.error).toBeNull();
    expect(pr.mergeBase).toBe(base);
    expect(pr.paths).toEqual(["docs/rewrite.md"]);
  });

  test("renames report old and new names; deletes are kept", () => {
    const repo = new Repo();
    repo.commit({ "keep.md": "keep\n", "apps/web/src/old.ts": "old\n".repeat(20) });
    const base = repo.commit({ "gone.ts": "gone\n" });
    repo.rename("apps/web/src/old.ts", "apps/web/src/new.ts");
    repo.remove("gone.ts");
    const pr = diffPathsForPr(repoGit(repo.dir), base, repo.head());
    expect(pr.error).toBeNull();
    expect(new Set(pr.paths)).toEqual(
      new Set(["apps/web/src/old.ts", "apps/web/src/new.ts", "gone.ts"]),
    );
  });

  test("renames between agent docs and into code", () => {
    const body = "role record line\n".repeat(20);
    for (const [from, to, reason] of [
      ["AGENTS.md", "notes/AGENTS.md", "NARROW_DOCS"],
      [".agents/skills/x/SKILL.md", ".agents/environment.md", "NARROW_DOCS"],
      ["src/env.md", "AGENTS.md", "NARROW_DOCS"],
      ["AGENTS.md", "Cargo.toml", "FULL_PATH_BROADEN"],
    ] as const) {
      const repo = new Repo();
      const base = repo.commit({ [from]: body });
      repo.rename(from, to);
      const pr = diffPathsForPr(repoGit(repo.dir), base, repo.head());
      expect(pr.paths).toEqual([from, to]);
      expect(decideFromPaths(pr.paths ?? []).reasonCode).toBe(reason);
    }
  });

  test("unresolvable refs and a failing diff fail closed", () => {
    const repo = new Repo();
    const base = repo.commit({ "AGENTS.md": "a\n" });
    // rev-parse echoes a well-formed unknown SHA; merge-base then refuses it.
    expect(diffPathsForPr(repoGit(repo.dir), base, "f".repeat(40))).toEqual({
      paths: null,
      error: "MERGE_BASE_FAILED",
      mergeBase: null,
    });
    expect(diffPathsForPr(repoGit(repo.dir), base, "F".repeat(40)).error).toBe("SHA_INVALID");
    expect(diffPathsForPr(repoGit(repo.dir), "HEAD", base).error).toBe("SHA_INVALID");
    const head = repo.commit({ ".agents/environment.md": "b\n" });
    const real = repoGit(repo.dir);
    const failing: Git = { ...real, diffPaths: () => ({ value: null, error: "GIT_DIFF_FAILED" }) };
    expect(diffPathsForPr(failing, base, head)).toEqual({
      paths: null,
      error: "GIT_DIFF_FAILED",
      mergeBase: base,
    });
  });
});

describe("pull request checkout binding", () => {
  test(
    "the exact merge narrows docs and frontend",
    () => {
      const fx = new PrCheckout();
      const docs = fx.branch({ "README.md": "docs only\n" });
      const m = fx.merge(fx.base, docs);
      const inputs = resolve(m.work, fx.base, docs, m.tested);
      expect(inputs).toMatchObject({
        paths: ["README.md"],
        fatalError: null,
        forceFullReason: null,
        mergeBaseSha: fx.base,
      });
      const frontend = fx.branch({ "apps/web/src/x.ts": "export {}\n" });
      const f = fx.merge(fx.base, frontend);
      expect(resolve(f.work, fx.base, frontend, f.tested).paths).toEqual(["apps/web/src/x.ts"]);
    },
    TIMEOUT,
  );

  test(
    "an unrelated merge, an older first parent or a direct head cannot narrow",
    () => {
      const fx = new PrCheckout();
      const docs = fx.branch({ "README.md": "docs only\n" });
      const code = fx.branch({ "src/lib.rs": "fn x() {}\n" });
      const wrong = fx.merge(fx.base, code);
      expect(resolve(wrong.work, fx.base, docs, wrong.tested)).toMatchObject({
        forceFullReason: "FULL_PR_MERGE_PARENTS_MISMATCH",
        paths: null,
      });
      const older = fx.merge(fx.base, docs);
      const eventBase = fx.advance({ "src/advanced.rs": "advance\n" });
      git(older.work, "fetch", "-q", "origin", "main");
      expect(resolve(older.work, eventBase, docs, older.tested).forceFullReason).toBe(
        "FULL_PR_MERGE_PARENTS_MISMATCH",
      );
      const direct = fx.clone();
      git(direct, "checkout", "-q", "--detach", docs);
      expect(resolve(direct, fx.base, docs, docs).forceFullReason).toBe(
        "FULL_PR_CHECKOUT_NOT_MERGE",
      );
      // A spoofed merge whose first parent is an unrelated root.
      const spoof = fx.clone();
      git(spoof, "checkout", "-q", "--orphan", "unrelated");
      git(spoof, "rm", "-rqf", ".");
      writeFile(spoof, "unrelated");
      git(spoof, "add", ".");
      git(spoof, "commit", "-q", "-m", "root");
      const tested = git(
        spoof,
        "commit-tree",
        `${docs}^{tree}`,
        "-p",
        git(spoof, "rev-parse", "HEAD"),
        "-p",
        docs,
        "-m",
        "spoof",
      );
      git(spoof, "checkout", "-q", "--detach", tested);
      expect(resolve(spoof, eventBase, docs, tested).forceFullReason).toBe(
        "FULL_PR_MERGE_PARENTS_MISMATCH",
      );
    },
    TIMEOUT,
  );

  test(
    "a base that advanced with the exact head can narrow and keeps the event base",
    () => {
      const fx = new PrCheckout();
      const docs = fx.branch({ "README.md": "docs only\n" });
      const advanced = fx.advance({ "src/lib.rs": "fn advanced() {}\n" });
      const m = fx.merge(advanced, docs);
      expect(resolve(m.work, fx.base, docs, m.tested)).toMatchObject({
        paths: ["README.md"],
        baseSha: fx.base,
        testedSha: m.tested,
      });
    },
    TIMEOUT,
  );

  test(
    "merge-only add, delete and rename are classified",
    () => {
      for (const operation of ["add", "delete", "rename"] as const) {
        const fx = new PrCheckout();
        const base = fx.advance({ "src/keep.rs": "base backend\n" });
        const head = fx.branch({ "README.md": "changed docs\n" });
        const m = fx.merge(base, head);
        const tested = fx.amend(m.work, () => {
          if (operation === "add") writeFile(m.work, "src/injected.rs", "merge only\n");
          else if (operation === "delete") rmSync(join(m.work, "src/keep.rs"));
          else {
            writeFile(m.work, "apps/web/src/disguised.ts", "base backend\n");
            rmSync(join(m.work, "src/keep.rs"));
          }
        });
        const inputs = resolve(m.work, base, head, tested);
        expect(inputs.fatalError).toBeNull();
        expect(inputs.forceFullReason).toBeNull();
        expect(inputs.paths).toContain("README.md");
        expect(decideFromPaths(inputs.paths ?? []).mode, operation).toBe("full");
      }
    },
    TIMEOUT,
  );

  test(
    "merge resolutions and cumulative head changes are both inspected",
    () => {
      const fx = new PrCheckout();
      const head = fx.branch({ "README.md": "changed docs\n" });
      const advanced = fx.advance({ "src/advanced.rs": "base backend\n" });
      const m = fx.merge(advanced, head);
      const tested = fx.amend(m.work, () => {
        writeFile(m.work, "src/advanced.rs", "merge resolution\n");
      });
      const inputs = resolve(m.work, fx.base, head, tested);
      expect(inputs.paths).toContain("src/advanced.rs");

      const fx2 = new PrCheckout();
      const backend = fx2.branch({ "src/new.rs": "backend change\n" });
      const m2 = fx2.merge(fx2.base, backend);
      const tested2 = fx2.amend(m2.work, () => {
        rmSync(join(m2.work, "src/new.rs"));
        writeFile(m2.work, "README.md", "merge omitted backend\n");
      });
      const omitted = resolve(m2.work, fx2.base, backend, tested2);
      expect(omitted.paths).toContain("src/new.rs");
      expect(decideFromPaths(omitted.paths ?? []).mode).toBe("full");
    },
    TIMEOUT,
  );

  test(
    "missing history, tested SHA mismatch and failing reads fail closed",
    () => {
      const fx = new PrCheckout();
      const head = fx.branch({ "README.md": "changed docs\n" });
      const m = fx.merge(fx.base, head);
      expect(resolve(m.work, fx.base, head, head).fatalError).toBe("TESTED_SHA_MISMATCH");
      expect(resolve(m.work, fx.base, head, "nope")).toMatchObject({
        fatalError: "TESTED_SHA_INVALID",
        testedSha: null,
      });
      // The real fetch from origin cannot find an unknown object.
      expect(resolve(m.work, "f".repeat(40), head, m.tested)).toMatchObject({
        fatalError: "FETCH_FAILED",
        paths: null,
      });
      // The second (actual merge) diff failing is fatal too.
      let calls = 0;
      const real = repoGit(m.work);
      const inputs = resolve(m.work, fx.base, head, m.tested, {
        diffPaths: (a, b) =>
          ++calls === 1 ? real.diffPaths(a, b) : { value: null, error: "GIT_DIFF_FAILED" },
      });
      expect(calls).toBe(2);
      expect(inputs).toMatchObject({ fatalError: "GIT_DIFF_FAILED", paths: null });
      expect(
        resolve(m.work, fx.base, head, m.tested, {
          revParse: () => ({ value: null, error: "REV_PARSE_FAILED" }),
        }).fatalError,
      ).toBe("HEAD_REV_PARSE_FAILED");
    },
    TIMEOUT,
  );

  test("only missing commits are fetched, by exact SHA", () => {
    const fetched: string[][] = [];
    const fake = {
      objectExists: (sha: string) => sha === SHA_A,
      fetchOrigin: (...refs: string[]) => (fetched.push(refs), null),
    } as unknown as Git;
    expect(ensureCommitShas(fake, SHA_A, SHA_B)).toBeNull();
    expect(fetched).toEqual([[SHA_B]]);
    expect(ensureCommitShas(fake, SHA_A)).toBeNull();
    expect(fetched).toHaveLength(1);
    expect(ensureCommitShas(fake, "main")).toBe("SHA_INVALID");
  });
});
