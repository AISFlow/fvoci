// Pre-browser network settle wait of one web e2e group (see
// settle_network_before_browser in scripts/web-e2e-inner.sh): wait until no
// IPv6 address is tentative and the group's netlink monitor log has been quiet
// for QUIET_S, at most LIMIT_S. Reads NET_MONITOR_LOG, NET_MARKS_LOG and
// NET_MONITOR_PID; reports on stderr only, each message also appended to the
// marks log. A check unavailable at the start is skipped with a message; one
// that fails after it ran once exits 1, so the group fails before the browser.
import { appendFileSync, readFileSync, statSync, writeSync } from "node:fs";

export const LIMIT_S = 10;
export const QUIET_S = 1;
export const POLL_S = 0.1;
const IP_TIMEOUT_MS = 5000;
// dadfailed addresses stay tentative forever; they never settle.
export const TENTATIVE_COMMAND = ["ip", "-6", "-o", "addr", "show", "tentative", "-dadfailed"];

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** The tentative-address query could not run or did not succeed. */
export class ProbeError extends Error {}

/** Interface names (second field, trailing ":" removed) of `ip -o addr` lines. */
export function tentativeInterfaces(output: string): string[] {
  const names = new Set<string>();
  for (const line of output.split("\n")) {
    const fields = line.trim().split(/\s+/);
    if (fields.length > 1) names.add((fields[1] ?? "").replace(/:+$/, ""));
  }
  return [...names].sort();
}

/** Lines in a file: newlines, plus an unterminated last line. */
export function countLines(bytes: Uint8Array): number {
  let count = 0;
  for (const byte of bytes) if (byte === 0x0a) count += 1;
  if (bytes.length > 0 && bytes[bytes.length - 1] !== 0x0a) count += 1;
  return count;
}

/** A marks-log line: "[UTC wall-clock time, microsecond field] # fvoci: network settle: …". */
export function markLine(epochMicros: bigint, message: string): string {
  const seconds = epochMicros / 1_000_000n;
  const micros = (epochMicros % 1_000_000n).toString().padStart(6, "0");
  const time = new Date(Number(seconds) * 1000).toISOString().slice(0, 19);
  return `[${time}.${micros}] # fvoci: network settle: ${message}\n`;
}

export interface SettleHost {
  /** Seconds on a monotonic clock. */
  monotonic(): number;
  /** Tentative interface names; throws ProbeError when the query fails. */
  listTentative(): string[];
  monitorRunning(): boolean;
  /** Seconds since the monitor log last changed. */
  quietSeconds(): number;
  eventCount(): number;
  sleep(seconds: number): void;
  say(message: string): void;
}

export function settle(host: SettleHost): void {
  const start = host.monotonic();
  let checkTentative = true;
  try {
    host.listTentative();
  } catch (error) {
    if (!(error instanceof ProbeError)) throw error;
    checkTentative = false;
    host.say(`cannot list tentative addresses (${error.message})`);
  }
  const watchEvents = host.monitorRunning();
  if (!watchEvents) host.say("netlink monitor not running; not checking for recent events");
  if (!checkTentative && !watchEvents) {
    host.say("skipped");
    return;
  }
  for (;;) {
    const elapsed = host.monotonic() - start;
    const tentative = checkTentative ? host.listTentative() : [];
    const quiet = watchEvents ? host.quietSeconds() : undefined;
    const events = watchEvents
      ? `; netlink events since the group started: ${String(host.eventCount())}`
      : "";
    if (tentative.length === 0 && (quiet === undefined || quiet >= QUIET_S)) {
      host.say(`settled after ${elapsed.toFixed(2)} s${events}`);
      return;
    }
    if (elapsed >= LIMIT_S) {
      let detail = `tentative: ${tentative.join(", ") || "none"}`;
      if (quiet !== undefined) detail += `; last netlink event ${quiet.toFixed(2)} s ago`;
      host.say(
        `warning: host network still changing after ${LIMIT_S.toFixed(0)} s (${detail}${events}); continuing`,
      );
      return;
    }
    host.sleep(POLL_S);
  }
}

function describeStderr(bytes: Uint8Array): string {
  let text: string;
  try {
    text = utf8.decode(bytes).trim();
  } catch {
    return ": stderr is not UTF-8";
  }
  return text === "" ? "" : `: ${text.split(/\s*\n\s*/).join(" / ")}`;
}

function listTentative(): string[] {
  const command = TENTATIVE_COMMAND.join(" ");
  let result;
  try {
    result = Bun.spawnSync(TENTATIVE_COMMAND, {
      stdin: "ignore",
      stdout: "pipe",
      stderr: "pipe",
      timeout: IP_TIMEOUT_MS,
      killSignal: "SIGKILL",
    });
  } catch (error) {
    const code = (error as { code?: unknown }).code;
    throw new ProbeError(
      `${command} could not start: ${typeof code === "string" ? code : String(error)}`,
    );
  }
  if (result.exitedDueToTimeout) {
    throw new ProbeError(`${command} timed out after ${String(IP_TIMEOUT_MS / 1000)} s`);
  }
  if (!result.success) {
    const how =
      result.signalCode === undefined
        ? `exited with status ${String(result.exitCode)}`
        : `was killed by ${result.signalCode}`;
    throw new ProbeError(`${command} ${how}${describeStderr(result.stderr)}`);
  }
  return tentativeInterfaces(utf8.decode(result.stdout));
}

// The wall clock, as ip -tshort and bash's EPOCHREALTIME markers use, so the
// merged timeline sorts; millisecond resolution in a microsecond field.
function epochMicros(): bigint {
  return BigInt(Date.now()) * 1000n;
}

function systemHost(env: NodeJS.ProcessEnv): SettleHost {
  const monitorLog = env.NET_MONITOR_LOG ?? "";
  const marksLog = env.NET_MARKS_LOG ?? "";
  const monitorPid = env.NET_MONITOR_PID ?? "";
  return {
    monotonic: () => performance.now() / 1000,
    listTentative,
    monitorRunning() {
      if (!/^[1-9][0-9]*$/.test(monitorPid) || monitorLog === "") return false;
      try {
        if (!statSync(monitorLog).isFile()) return false;
        process.kill(Number(monitorPid), 0);
      } catch {
        return false;
      }
      return true;
    },
    quietSeconds: () => (Date.now() - statSync(monitorLog).mtimeMs) / 1000,
    eventCount: () => countLines(readFileSync(monitorLog)),
    sleep(seconds) {
      Bun.sleepSync(seconds * 1000);
    },
    say(message) {
      writeSync(2, `network settle: ${message}\n`);
      if (marksLog !== "") appendFileSync(marksLog, markLine(epochMicros(), message));
    },
  };
}

if (import.meta.main) {
  if (process.argv.length > 2) {
    process.stderr.write(
      "usage: NET_MONITOR_LOG=… NET_MARKS_LOG=… NET_MONITOR_PID=… network-settle.ts\n",
    );
    process.exit(2);
  }
  try {
    settle(systemHost(process.env));
  } catch (error) {
    writeSync(
      2,
      `network settle: error: ${error instanceof Error ? error.message : String(error)}\n`,
    );
    process.exit(1);
  }
}
