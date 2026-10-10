// Git reads the planner trusts: exact SHAs, merge bases, parents and the
// strict -z name-status diff. Every failure is a reason code, never a guess.

// Python `re.match("^...$")` also accepts one trailing newline.
export const SHA_RE = /^[0-9a-f]{40}\n?$/;

export function validateSha(ref: string): boolean {
  return SHA_RE.test(ref);
}

type Result<T> = { value: T; error: null } | { value: null; error: string };
const ok = <T>(value: T): Result<T> => ({ value, error: null });
const err = <T>(error: string): Result<T> => ({ value: null, error });

const STRICT_UTF8 = new TextDecoder("utf-8", { fatal: true });

/** Strict `git diff --name-status -z` parser; rejects truncated records. */
export function parseNameStatusZ(data: Uint8Array): Result<string[]> {
  if (data.length > 0 && data[data.length - 1] !== 0) return err("DIFF_TRUNCATED");
  const fields: string[] = [];
  let start = 0;
  for (let i = 0; i < data.length; i++) {
    if (data[i] === 0) {
      // A path that is not UTF-8 is refused outright (it cannot be classified).
      fields.push(STRICT_UTF8.decode(data.subarray(start, i)));
      start = i + 1;
    }
  }
  const paths: string[] = [];
  let i = 0;
  while (i < fields.length) {
    const status = fields[i++] as string;
    if (!status) return err("DIFF_EMPTY_STATUS");
    if (!/^[ACDMRTU][0-9]*$/.test(status)) return err("DIFF_BAD_STATUS");
    const kind = status[0];
    if (kind === "R" || kind === "C") {
      if (i + 1 >= fields.length) return err("DIFF_TRUNCATED_RENAME");
      paths.push(fields[i] as string, fields[i + 1] as string);
      i += 2;
    } else {
      if (i >= fields.length)
        return err(kind === "D" ? "DIFF_TRUNCATED_DELETE" : "DIFF_TRUNCATED_PATH");
      paths.push(fields[i++] as string);
    }
  }
  return ok(paths);
}

// `code` is 0 only for a clean exit; a signal or spawn failure is nonzero.
export type GitRun = { code: number; stdout: Uint8Array };
export type GitRunner = (repo: string, args: readonly string[]) => GitRun;

export const spawnGit: GitRunner = (repo, args) => {
  const proc = Bun.spawnSync(["git", ...args], {
    cwd: repo,
    stdout: "pipe",
    stderr: "pipe",
    stdin: "ignore",
  });
  return { code: proc.success ? 0 : 1, stdout: proc.stdout };
};

const text = (run: GitRun) => STRICT_UTF8.decode(run.stdout).trim();

/** The git reads selection depends on; tests replace single methods. */
export type Git = {
  revParse(ref: string, requireShaRef?: boolean): Result<string>;
  mergeBase(a: string, b: string): Result<string>;
  diffPaths(base: string, head: string): Result<string[]>;
  objectExists(sha: string): boolean;
  fetchOrigin(...refs: string[]): string | null;
  commitParents(sha: string): Result<string[]>;
  isAncestor(ancestor: string, descendant: string): boolean;
};

export function repoGit(repo: string, run: GitRunner = spawnGit): Git {
  const git = (...args: string[]) => run(repo, args);
  return {
    revParse(ref, requireShaRef = true) {
      if (requireShaRef && !validateSha(ref)) return err("SHA_INVALID");
      const proc = git("rev-parse", ref);
      if (proc.code !== 0) return err("REV_PARSE_FAILED");
      const sha = text(proc);
      return validateSha(sha) ? ok(sha) : err("SHA_INVALID");
    },
    mergeBase(a, b) {
      const proc = git("merge-base", a, b);
      if (proc.code !== 0) return err("MERGE_BASE_FAILED");
      const sha = text(proc);
      return validateSha(sha) ? ok(sha) : err("SHA_INVALID");
    },
    diffPaths(base, head) {
      const proc = git("diff", "--name-status", "-z", "-M", base, head);
      if (proc.code !== 0) return err("GIT_DIFF_FAILED");
      return parseNameStatusZ(proc.stdout);
    },
    objectExists(sha) {
      return git("cat-file", "-e", `${sha}^{commit}`).code === 0;
    },
    fetchOrigin(...refs) {
      if (!refs.every(validateSha)) return "SHA_INVALID";
      return git("fetch", "--no-tags", "origin", ...refs).code === 0 ? null : "FETCH_FAILED";
    },
    commitParents(sha) {
      if (!validateSha(sha)) return err("SHA_INVALID");
      const proc = git("rev-list", "--parents", "-n", "1", sha);
      if (proc.code !== 0) return err("REV_LIST_PARENTS_FAILED");
      const parts = text(proc).split(/\s+/).filter(Boolean);
      if (parts.length === 0) return err("REV_LIST_PARENTS_EMPTY");
      if (parts[0] !== sha) return err("REV_LIST_COMMIT_MISMATCH");
      const parents = parts.slice(1);
      return parents.every(validateSha) ? ok(parents) : err("SHA_INVALID");
    },
    isAncestor(ancestor, descendant) {
      return git("merge-base", "--is-ancestor", ancestor, descendant).code === 0;
    },
  };
}

/** Cumulative PR paths: merge-base(base, head)..head. */
export function diffPathsForPr(
  git: Git,
  baseSha: string,
  headSha: string,
): { paths: string[] | null; error: string | null; mergeBase: string | null } {
  const base = git.revParse(baseSha);
  if (base.error !== null) return { paths: null, error: base.error, mergeBase: null };
  const head = git.revParse(headSha);
  if (head.error !== null) return { paths: null, error: head.error, mergeBase: null };
  const mergeBase = git.mergeBase(base.value, head.value);
  if (mergeBase.error !== null) return { paths: null, error: mergeBase.error, mergeBase: null };
  const diff = git.diffPaths(mergeBase.value, head.value);
  if (diff.error !== null) return { paths: null, error: diff.error, mergeBase: mergeBase.value };
  return { paths: diff.value, error: null, mergeBase: mergeBase.value };
}

export function ensureCommitShas(git: Git, ...shas: string[]): string | null {
  if (!shas.every(validateSha)) return "SHA_INVALID";
  const missing = shas.filter((sha) => !git.objectExists(sha));
  return missing.length === 0 ? null : git.fetchOrigin(...missing);
}
