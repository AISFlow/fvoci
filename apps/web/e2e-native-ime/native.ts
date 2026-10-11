import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import type { Page, Locator } from "@playwright/test";
import { browserProcesses } from "./proc";

const session = process.env.FVOCI_NATIVE_IME_SESSION;
if (!session) throw new Error("FVOCI_NATIVE_IME_SESSION is required for native evidence");
export const evidence = session;

export type NativeImeEvent = {
  type: string;
  time: number;
  key?: string;
  code?: string;
  keyCode?: number;
  which?: number;
  data?: string | null;
  inputType?: string;
  isComposing?: boolean;
  isTrusted: boolean;
  defaultPrevented: boolean;
  defaultPreventedAfterDispatch: boolean;
  value: string | null;
};
declare global {
  interface Window {
    nativeImeEvents?: NativeImeEvent[];
  }
}
export function nativeImeEvents(page: Page): Promise<NativeImeEvent[]> {
  return page.evaluate(() => {
    const events = window.nativeImeEvents;
    if (!events) throw new Error("native IME observation has not been installed");
    return events;
  });
}
export function native(command: string, args: string[]) {
  if (args.includes("--window")) throw new Error("XSendEvent targeting is forbidden");
  const out = execFileSync(command, args, { encoding: "utf8" });
  writeFileSync(
    join(evidence, "native-commands.jsonl"),
    JSON.stringify({ time: new Date().toISOString(), command, args, out }) + "\n",
    { flag: "a" },
  );
  return out.trim();
}
export function keys(...keys: string[]) {
  native("xdotool", ["key", "--delay", "180", ...keys]);
}
export async function observe(page: Page) {
  await page.evaluate(() => {
    window.nativeImeEvents = [];
    for (const type of [
      "keydown",
      "keyup",
      "compositionstart",
      "compositionupdate",
      "compositionend",
      "beforeinput",
      "input",
    ]) {
      document.addEventListener(
        type,
        (e: Event) => {
          if (!(e.target instanceof Node)) throw new Error("native input target is not a DOM node");
          // These legacy numeric fields are evidence of the actual OS dispatch,
          // including IME keyCode 229; modern key/code cannot replace that witness.
          const keyCode: unknown =
            e instanceof KeyboardEvent ? Reflect.get(e, "keyCode") : undefined;
          const which: unknown = e instanceof UIEvent ? Reflect.get(e, "which") : undefined;
          if (
            (e instanceof KeyboardEvent && typeof keyCode !== "number") ||
            (e instanceof UIEvent && typeof which !== "number")
          ) {
            throw new Error("native keyboard event lacks numeric legacy evidence");
          }
          const record: NativeImeEvent = {
            type,
            time: performance.now(),
            key: e instanceof KeyboardEvent ? e.key : undefined,
            code: e instanceof KeyboardEvent ? e.code : undefined,
            keyCode: typeof keyCode === "number" ? keyCode : undefined,
            which: typeof which === "number" ? which : undefined,
            data: e instanceof InputEvent || e instanceof CompositionEvent ? e.data : undefined,
            inputType: e instanceof InputEvent ? e.inputType : undefined,
            isComposing:
              e instanceof InputEvent || e instanceof KeyboardEvent ? e.isComposing : undefined,
            isTrusted: e.isTrusted,
            defaultPrevented: e.defaultPrevented,
            defaultPreventedAfterDispatch: e.defaultPrevented,
            value:
              e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement
                ? e.target.value
                : e.target.textContent,
          };
          const events = window.nativeImeEvents;
          if (!events) throw new Error("native IME event log is missing");
          events.push(record);
          // Native event dispatch can run microtasks between listeners; observe
          // cancellation in the next task, after all editor listeners ran.
          setTimeout(() => {
            record.defaultPreventedAfterDispatch = e.defaultPrevented;
          }, 0);
        },
        true,
      );
    }
  });
}
export async function snapshot(page: Page, name: string) {
  writeFileSync(
    join(evidence, name + ".json"),
    JSON.stringify(
      await page.evaluate(() => ({
        url: location.href,
        events: window.nativeImeEvents,
        active: document.activeElement?.outerHTML,
        selection: window.getSelection()?.toString(),
      })),
      null,
      2,
    ),
  );
  await page.screenshot({ path: join(evidence, name + ".png") });
}
export async function focusNative(page: Page, field: Locator, profile: string) {
  const candidates = browserProcesses(profile);
  if (candidates.length !== 1)
    throw new Error("Ambiguous owned browser process: " + JSON.stringify(candidates));
  const candidate = candidates[0];
  if (!candidate) throw new Error("owned browser candidate missing");
  if (!candidate.exe.endsWith("/chrome-linux64/chrome"))
    throw new Error("Owned Chrome executable mismatch");
  // Chromium rewrites its process title and /proc/environ may be empty.
  // Verify the browser on this X connection by its XID and _NET_WM_PID.
  if (candidate.display && candidate.display !== process.env.DISPLAY)
    throw new Error("Owned Chrome display mismatch");
  const { pid } = candidate;
  const windows = native("xdotool", ["search", "--onlyvisible", "--pid", pid])
    .split("\n")
    .filter(Boolean);
  const title = await page.title();
  const owned = windows.filter((id) =>
    native("xdotool", ["getwindowname", id]).startsWith(title + " - "),
  );
  if (owned.length !== 1) throw new Error("Ambiguous Chrome XID: " + JSON.stringify(windows));
  const xid = owned[0];
  if (!xid) throw new Error("owned Chrome XID missing");
  const windowProperties = native("xprop", ["-id", xid, "_NET_WM_PID", "WM_CLASS", "WM_NAME"]);
  if (!windowProperties.includes(`_NET_WM_PID(CARDINAL) = ${pid}`))
    throw new Error("XID PID does not match owned Chrome");
  writeFileSync(
    join(evidence, "browser-ownership.json"),
    JSON.stringify(
      {
        ...candidate,
        displayFromProc: candidate.display,
        xid,
        windows,
        profile,
        display: process.env.DISPLAY,
        displayProof: "owned PID and XID queried through this private X connection",
        windowProperties,
      },
      null,
      2,
    ),
  );
  native("xdotool", ["windowfocus", "--sync", xid]);
  await snapshot(page, "before-focus");
  await clickNative(page, field);
  if (!(await field.evaluate((el) => el === document.activeElement)))
    throw new Error("Native click did not focus field");
}
export async function clickNative(page: Page, field: Locator) {
  const rect = await field.boundingBox();
  if (!rect) throw new Error("No visible field");
  const frame = await page.evaluate(() => ({
    x: window.screenX,
    y: window.screenY,
    top: window.outerHeight - window.innerHeight,
  }));
  native("xdotool", [
    "mousemove",
    String(Math.round(frame.x + rect.x + 8)),
    String(Math.round(frame.y + frame.top + rect.y + 12)),
  ]);
  native("xdotool", ["click", "1"]);
}
