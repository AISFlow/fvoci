// A refusal with the process exit code it maps to. Only the CLI entry point
// turns it into stderr and an exit status.

export const EXIT_FAILED = 1;
export const EXIT_USAGE = 2;
/** The registry refused to answer (401/403); callers branch on this code. */
export const EXIT_UNREADABLE = 4;

export class Fail extends Error {
  constructor(
    message: string,
    readonly code: number = EXIT_FAILED,
  ) {
    super(message);
  }
}

/** Python-style repr of a value, so refusal text names inputs unambiguously. */
export function repr(value: unknown): string {
  if (value === undefined || value === null) return "None";
  if (typeof value === "boolean") return value ? "True" : "False";
  if (typeof value === "string") {
    const quote = value.includes("'") && !value.includes('"') ? '"' : "'";
    let out = quote;
    for (const ch of value) {
      const code = ch.codePointAt(0) ?? 0;
      if (ch === "\\" || ch === quote) out += `\\${ch}`;
      else if (ch === "\n") out += "\\n";
      else if (ch === "\r") out += "\\r";
      else if (ch === "\t") out += "\\t";
      else if (code < 0x20 || code === 0x7f) out += `\\x${code.toString(16).padStart(2, "0")}`;
      else out += ch;
    }
    return out + quote;
  }
  if (Array.isArray(value)) return `[${value.map(repr).join(", ")}]`;
  if (typeof value === "number" || typeof value === "bigint") return String(value);
  return JSON.stringify(value);
}

/** str() of a value as Python prints it in an f-string. */
export function str(value: unknown): string {
  return typeof value === "string" ? value : repr(value);
}

/** JSON from response bytes (strict UTF-8) or text; a parse error is a refusal naming `what`. */
export function parseJson(input: Uint8Array | string, what = "response"): unknown {
  try {
    const text =
      typeof input === "string" ? input : new TextDecoder("utf-8", { fatal: true }).decode(input);
    return JSON.parse(text);
  } catch {
    throw new Fail(`${what} is not JSON`);
  }
}
