// Linux /proc scans that identify the processes owned by one native IME run.
// A process that vanishes, denies access, or carries non-UTF-8 cmdline/environ
// bytes is skipped, never guessed at.
import { readdirSync, readFileSync, readlinkSync } from "node:fs";
import { join } from "node:path";

class InvalidUtf8 extends Error {}
// ignoreBOM keeps a leading U+FEFF: in argv and environ it is data, not encoding metadata.
const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
function decode(bytes: Uint8Array): string {
  try {
    return utf8.decode(bytes);
  } catch {
    throw new InvalidUtf8();
  }
}

/** NUL-separated KEY=VALUE pairs; entries without "=" are ignored, later keys win. */
export function parseEnviron(text: string): Map<string, string> {
  const env = new Map<string, string>();
  for (const entry of text.split("\0")) {
    const eq = entry.indexOf("=");
    if (eq >= 0) env.set(entry.slice(0, eq), entry.slice(eq + 1));
  }
  return env;
}

/** Field 22 (starttime) of /proc/<pid>/stat, counted after the parenthesised comm. */
export function startTicks(stat: string): string {
  const fields = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
  const ticks = fields[19];
  if (ticks === undefined || !/^\d+$/.test(ticks)) throw new Error("malformed /proc stat");
  return ticks;
}

/**
 * POSIX shell-word split (Python shlex.split semantics) for a process title
 * that was rewritten into a single argv string. Unbalanced quotes throw.
 */
export function shellSplit(text: string): string[] {
  const words: string[] = [];
  let word = "";
  let inWord = false;
  let quote: "'" | '"' | null = null;
  for (let i = 0; i < text.length; i++) {
    const c = text.charAt(i);
    if (quote === "'") {
      if (c === "'") quote = null;
      else word += c;
    } else if (quote === '"') {
      if (c === '"') quote = null;
      else if (c === "\\") {
        const next = text.charAt(++i);
        if (i >= text.length) throw new Error("No escaped character");
        word += next === '"' || next === "\\" ? next : c + next;
      } else word += c;
    } else if (c === " " || c === "\t" || c === "\r" || c === "\n") {
      if (inWord) words.push(word);
      word = "";
      inWord = false;
    } else {
      inWord = true;
      if (c === "'" || c === '"') quote = c;
      else if (c === "\\") {
        if (++i >= text.length) throw new Error("No escaped character");
        word += text.charAt(i);
      } else word += c;
    }
  }
  if (quote) throw new Error("No closing quotation");
  if (inWord) words.push(word);
  return words;
}

/** JSON.stringify with Python json.dumps(ensure_ascii=True) escaping, for byte-stable evidence. */
export function asciiJson(value: unknown): string {
  return JSON.stringify(value, null, 2).replace(
    /[\u007f-\uffff]/g,
    (c) => "\\u" + c.charCodeAt(0).toString(16).padStart(4, "0"),
  );
}

// A process that exited (ENOENT/ESRCH) or belongs to someone else (EACCES/EPERM).
const vanishedOrForeign = new Set(["ENOENT", "ESRCH", "EACCES", "EPERM"]);
function isSkippable(error: unknown): boolean {
  if (error instanceof InvalidUtf8) return true;
  const code: unknown = error instanceof Error ? Reflect.get(error, "code") : undefined;
  return typeof code === "string" && vanishedOrForeign.has(code);
}

/** Run `inspect` on every numeric /proc entry, skipping vanished or unreadable processes. */
function scan<T>(inspect: (dir: string, pid: string) => T | undefined): T[] {
  const found: T[] = [];
  for (const pid of readdirSync("/proc")) {
    if (!/^\d+$/.test(pid)) continue;
    try {
      const entry = inspect(join("/proc", pid), pid);
      if (entry !== undefined) found.push(entry);
    } catch (error) {
      if (!isSkippable(error)) throw error;
    }
  }
  return found;
}

const environ = (dir: string) => parseEnviron(decode(readFileSync(join(dir, "environ"))));

export type FixtureProcess = {
  pid: number;
  exe: string;
  command: string;
  display: string | null;
  start_ticks: string;
};

/** Every process whose environment carries this FVOCI_NATIVE_IME_SESSION. */
export function sessionProcesses(session: string): FixtureProcess[] {
  return scan((dir, pid) => {
    const env = environ(dir);
    if (env.get("FVOCI_NATIVE_IME_SESSION") !== session) return undefined;
    return {
      pid: Number(pid),
      exe: readlinkSync(join(dir, "exe")),
      command: decode(readFileSync(join(dir, "cmdline"))).replaceAll("\0", " "),
      display: env.get("DISPLAY") ?? null,
      start_ticks: startTicks(readFileSync(join(dir, "stat"), "latin1")),
    };
  });
}

export type BrowserProcess = { pid: string; args: string[]; exe: string; display: string | null };

/** Browser (non `--type=` child) processes launched with exactly this user data dir. */
export function browserProcesses(profile: string): BrowserProcess[] {
  const flag = "--user-data-dir=" + profile;
  return scan((dir, pid) => {
    let args = decode(readFileSync(join(dir, "cmdline"))).split("\0");
    const title = args[0] ?? "";
    // An unparseable title is fatal only when it could carry a profile flag; an
    // unrelated host process must not abort the scan.
    if (args.filter(Boolean).length === 1) {
      if (!title.includes("--user-data-dir")) return undefined;
      args = shellSplit(title);
    }
    if (!args.includes(flag) || args.some((a) => a.startsWith("--type="))) return undefined;
    const env = environ(dir);
    return { pid, args, exe: readlinkSync(join(dir, "exe")), display: env.get("DISPLAY") ?? null };
  });
}
