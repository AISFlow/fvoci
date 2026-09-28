// Opt-in user-perceived performance probe (not part of the required e2e suite).
//
// Clock rules:
// - Every in-page timestamp is that page's performance.now() (ms since its
//   timeOrigin). Values from two pages are never subtracted directly.
// - Cross-page intervals convert each page's absolute time
//   (timeOrigin + now) to the Node runner clock through `calibrate()`, a
//   min-RTT round-trip alignment whose ±RTT/2 bound is kept with the sample.
// - "paint" values come only from Element Timing / Event Timing / Paint Timing
//   entries (presentation timestamps). DOM observation is reported as `dom`
//   and the following animation frame as `raf`; neither is called paint.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { performance as nodePerformance } from "node:perf_hooks";
import type { BrowserContext, Page } from "@playwright/test";

export type EventEntry = {
  n: string;
  s: number;
  ps: number;
  pe: number;
  d: number;
  i: number;
};
export type ElementEntry = { id: string; r: number; l: number };
export type InputMark = { t: string; ts: number; key?: string };
export type Hit = {
  dom: number;
  abs: number;
  /** requestAnimationFrame after the DOM hit: start of the frame that renders it (not paint). */
  raf?: number;
  pre?: boolean;
  tag?: string;
};
export type ResourceEntry = {
  name: string;
  type: string;
  start: number;
  reqStart: number;
  respStart: number;
  end: number;
  bytes: number;
};

type ProbeState = {
  ev: EventEntry[];
  el: ElementEntry[];
  paint: { n: string; s: number }[];
  inputs: InputMark[];
  hits: Record<string, Hit>;
  watch: (id: string, spec: WatchSpec) => void;
};

export type WatchSpec = {
  selector: string;
  text?: string;
  attr?: [string, string];
  /** canvas: require non-blank pixels before counting the hit. */
  canvas?: boolean;
};

declare global {
  interface Window {
    __fp?: ProbeState;
  }
}

// Runs in the page before app scripts. Must stay self-contained.
function installProbe(): void {
  if (window.__fp) return;
  const st = {
    ev: [] as EventEntry[],
    el: [] as ElementEntry[],
    paint: [] as { n: string; s: number }[],
    inputs: [] as InputMark[],
    hits: {} as Record<string, Hit>,
    watch: (_id: string, _spec: WatchSpec) => {},
  };
  window.__fp = st;
  const observe = (type: string, opts: Record<string, unknown>, fn: (e: PerformanceEntry) => void) => {
    try {
      new PerformanceObserver((list) => list.getEntries().forEach(fn)).observe({
        type,
        buffered: true,
        ...opts,
      } as PerformanceObserverInit);
    } catch {
      /* entry type unsupported: absence is reported, not faked */
    }
  };
  observe("event", { durationThreshold: 16 }, (raw) => {
    const e = raw as PerformanceEventTiming & { interactionId?: number };
    st.ev.push({
      n: e.name,
      s: e.startTime,
      ps: e.processingStart,
      pe: e.processingEnd,
      d: e.duration,
      i: e.interactionId ?? 0,
    });
  });
  observe("element", {}, (raw) => {
    const e = raw as PerformanceEntry & { identifier: string; renderTime: number; loadTime: number };
    st.el.push({ id: e.identifier, r: e.renderTime, l: e.loadTime });
  });
  observe("paint", {}, (e) => st.paint.push({ n: e.name, s: e.startTime }));
  for (const t of ["keydown", "pointerdown", "click", "beforeinput", "input"]) {
    addEventListener(
      t,
      (e) => {
        // Only the key identity of non-printable keys; typed text is never recorded.
        const key = e instanceof KeyboardEvent && e.key.length > 1 ? e.key : undefined;
        st.inputs.push({ t, ts: e.timeStamp, ...(key ? { key } : {}) });
      },
      { capture: true, passive: true },
    );
  }

  const canvasPainted = (el: Element): boolean => {
    if (!(el instanceof HTMLCanvasElement) || el.width === 0 || el.height === 0) return false;
    try {
      const ctx = el.getContext("2d");
      if (!ctx) return false;
      const w = Math.min(el.width, 64);
      const h = Math.min(el.height, 64);
      const x = Math.floor((el.width - w) / 2);
      const y = Math.floor((el.height - h) / 2);
      const px = ctx.getImageData(x, y, w, h).data;
      const r0 = px[0];
      const g0 = px[1];
      const b0 = px[2];
      for (let i = 4; i < px.length; i += 4) {
        if (px[i] !== r0 || px[i + 1] !== g0 || px[i + 2] !== b0) return true;
      }
      return false;
    } catch {
      return false;
    }
  };

  // Element Timing only reports an element whose `elementtiming` attribute is
  // present at its first paint, so the attribute goes on the node that
  // directly holds the matched text (or the img) inside the mutation callback,
  // which runs before the next rendering update.
  // Chromium aggregates text paint on the nearest block-level container, so an
  // inline holder (strong/span/a) hands the attribute up to that container.
  const blockOf = (node: Element): Element => {
    let cur: Element | null = node;
    while (cur && cur !== document.body) {
      const display = getComputedStyle(cur).display;
      if (!display.startsWith("inline") && display !== "contents") return cur;
      cur = cur.parentElement;
    }
    return node;
  };
  const timingTarget = (el: Element, text?: string): Element => {
    if (el instanceof HTMLImageElement) return el;
    const img = el.querySelector("img");
    if (!text && img) return img;
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const value = node.nodeValue ?? "";
      if (value.trim() && (!text || value.includes(text))) return blockOf(node.parentElement ?? el);
    }
    return blockOf(el);
  };

  st.watch = (id, spec) => {
    delete st.hits[id];
    const match = (): Element | null => {
      for (const el of document.querySelectorAll(spec.selector)) {
        if (spec.text && !(el.textContent ?? "").includes(spec.text)) continue;
        if (spec.attr && el.getAttribute(spec.attr[0]) !== spec.attr[1]) continue;
        if (spec.canvas && !canvasPainted(el)) continue;
        return el;
      }
      return null;
    };
    const hit = (el: Element, pre: boolean) => {
      const now = performance.now();
      const rec: Hit = { dom: now, abs: performance.timeOrigin + now, tag: el.tagName.toLowerCase() };
      if (pre) rec.pre = true;
      st.hits[id] = rec;
      const target = timingTarget(el, spec.text);
      if (!target.hasAttribute("elementtiming")) target.setAttribute("elementtiming", id);
      requestAnimationFrame(() => {
        rec.raf = performance.now();
      });
    };
    const first = match();
    if (first) {
      hit(first, true);
      return;
    }
    let done = false;
    const mo = new MutationObserver(() => {
      if (done) return;
      const el = match();
      if (el) {
        done = true;
        mo.disconnect();
        hit(el, false);
      }
    });
    mo.observe(document, { subtree: true, childList: true, characterData: true, attributes: true });
    if (spec.canvas) {
      // Canvas pixels change without DOM mutations: sample once per frame.
      const tick = () => {
        if (done) return;
        const el = match();
        if (el) {
          done = true;
          mo.disconnect();
          hit(el, false);
          return;
        }
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    }
  };
}

export async function attachProbe(target: BrowserContext | Page): Promise<void> {
  await target.addInitScript(installProbe);
}

export function nodeNow(): number {
  return nodePerformance.timeOrigin + nodePerformance.now();
}

export type Calibration = { offset: number; rtt: number; at: number };

/** Aligns the page clock (timeOrigin + now) to the Node clock; keeps the min-RTT round. */
export async function calibrate(page: Page, rounds = 15): Promise<Calibration> {
  let best: Calibration | null = null;
  for (let i = 0; i < rounds; i += 1) {
    const t0 = nodeNow();
    const p = await page.evaluate(() => performance.timeOrigin + performance.now());
    const t1 = nodeNow();
    const rtt = t1 - t0;
    if (!best || rtt < best.rtt) best = { offset: p - (t0 + t1) / 2, rtt, at: t1 };
  }
  return best!;
}

export async function watch(page: Page, id: string, spec: WatchSpec): Promise<void> {
  await page.evaluate(([i, s]) => window.__fp!.watch(i, s), [id, spec] as const);
}

/** Waits for a watch to fire. The timeout bounds a failure; it is never a result. */
export async function waitHit(page: Page, id: string, timeoutMs: number): Promise<Hit | null> {
  try {
    await page.waitForFunction((i) => Boolean(window.__fp?.hits[i]?.raf), id, {
      timeout: timeoutMs,
      polling: 16,
    });
  } catch {
    return null;
  }
  return (await page.evaluate((i) => window.__fp!.hits[i], id)) ?? null;
}

/** Waits (bounded) for the Element Timing entry of a watch; null when none arrives. */
export async function elementPaint(page: Page, id: string, timeoutMs = 1000): Promise<number | null> {
  try {
    await page.waitForFunction((i) => window.__fp!.el.some((e) => e.id === i), id, {
      timeout: timeoutMs,
      polling: 16,
    });
  } catch {
    return null;
  }
  return page.evaluate((i) => {
    const e = window.__fp!.el.find((x) => x.id === i)!;
    return e.r || e.l || null;
  }, id);
}

export async function pageNow(page: Page): Promise<number> {
  return page.evaluate(() => performance.now());
}

export async function inputsSince(page: Page, since: number): Promise<InputMark[]> {
  return page.evaluate((s) => window.__fp!.inputs.filter((x) => x.ts >= s), since);
}

export async function eventsSince(page: Page, since: number): Promise<EventEntry[]> {
  return page.evaluate((s) => window.__fp!.ev.filter((x) => x.s >= s), since);
}

export async function paints(page: Page): Promise<{ n: string; s: number }[]> {
  return page.evaluate(() => window.__fp!.paint);
}

const UUID_RE = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi;

/** Resource Timing entries since `since`; query strings dropped, ids masked. */
export async function resourcesSince(page: Page, since: number): Promise<ResourceEntry[]> {
  const raw = await page.evaluate(
    (s) =>
      (performance.getEntriesByType("resource") as PerformanceResourceTiming[])
        .filter((e) => e.startTime >= s)
        .map((e) => ({
          name: e.name,
          type: e.initiatorType,
          start: e.startTime,
          reqStart: e.requestStart,
          respStart: e.responseStart,
          end: e.responseEnd,
          bytes: e.encodedBodySize,
        })),
    since,
  );
  return raw.map((e) => ({ ...e, name: sanitizeUrl(e.name) }));
}

export function sanitizeUrl(url: string): string {
  try {
    const u = new URL(url);
    return u.pathname.replace(UUID_RE, ":id").replace(/\/assets\/[^/]+$/, "/assets/:file");
  } catch {
    return "(unparsed)";
  }
}

/** INP-style duration per interaction: max Event Timing duration per interactionId. */
export function interactions(entries: EventEntry[]): { id: number; duration: number; inputDelay: number }[] {
  const byId = new Map<number, { duration: number; inputDelay: number }>();
  for (const e of entries) {
    if (!e.i) continue;
    const prev = byId.get(e.i);
    const delay = e.ps - e.s;
    if (!prev || e.d > prev.duration) byId.set(e.i, { duration: e.d, inputDelay: delay });
  }
  return [...byId.entries()].map(([id, v]) => ({ id, ...v }));
}

export type Stats = { n: number; failures: number; median: number | null; p95: number | null; max: number | null };

export function stats(values: (number | null | undefined)[]): Stats {
  const ok = values.filter((v): v is number => typeof v === "number" && Number.isFinite(v)).sort((a, b) => a - b);
  const failures = values.length - ok.length;
  const q = (p: number) => (ok.length ? ok[Math.min(ok.length - 1, Math.ceil(p * ok.length) - 1)] : null);
  return {
    n: values.length,
    failures,
    median: ok.length ? (q(0.5) ?? null) : null,
    p95: ok.length ? (q(0.95) ?? null) : null,
    max: ok.length ? (ok[ok.length - 1] ?? null) : null,
  };
}

// ---- contention guard ----------------------------------------------------

export type LoadWindow = {
  label: string;
  startedAt: string;
  waitedMs: number;
  quiet: boolean;
  load1: number;
  load5: number;
  busy: string[];
};

function busyBuilds(): string[] {
  let out = "";
  try {
    out = execFileSync("ps", ["-eo", "pid=,args="], { encoding: "utf8" });
  } catch {
    return ["ps-unavailable"];
  }
  const busy: string[] = [];
  for (const line of out.split("\n")) {
    const m = line.trim().match(/^(\d+)\s+(.*)$/);
    if (!m) continue;
    const args = m[2] ?? "";
    const exe = path.basename(args.split(/\s+/)[0] ?? "");
    if (exe === "cargo" || exe === "rustc" || /\bdocker(-buildx)?\s+(buildx\s+)?build\b/.test(args)) {
      busy.push(exe === "cargo" || exe === "rustc" ? exe : "docker-build");
    }
  }
  return busy;
}

/**
 * Before a measurement window: wait (bounded) until no cargo/rustc/docker
 * build runs on this host and load1 is below the core count; record the state
 * either way so contended windows stay labelled instead of dropped.
 */
export async function quietWindow(label: string, log: LoadWindow[]): Promise<LoadWindow> {
  const maxWaitMs = Number(process.env.FVOCI_PERF_QUIET_MAX_MS ?? 300_000);
  const cores = os.cpus().length;
  const started = Date.now();
  let busy = busyBuilds();
  const loads = () => os.loadavg() as [number, number, number];
  let [load1, load5] = loads();
  while ((busy.length > 0 || load1 >= cores) && Date.now() - started < maxWaitMs) {
    await new Promise((resolve) => setTimeout(resolve, 5_000));
    busy = busyBuilds();
    [load1, load5] = loads();
  }
  const counts = new Map<string, number>();
  for (const b of busy) counts.set(b, (counts.get(b) ?? 0) + 1);
  const entry: LoadWindow = {
    label,
    startedAt: new Date().toISOString(),
    waitedMs: Date.now() - started,
    quiet: busy.length === 0 && load1 < cores,
    load1: Number(load1.toFixed(2)),
    load5: Number(load5.toFixed(2)),
    busy: [...counts.entries()].map(([k, v]) => `${k}x${v}`),
  };
  log.push(entry);
  return entry;
}

// ---- output ----------------------------------------------------------------

export function outDir(): string {
  const dir = process.env.FVOCI_PERF_OUT;
  if (!dir) throw new Error("FVOCI_PERF_OUT is required");
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

export function writeJson(name: string, value: unknown): void {
  fs.writeFileSync(path.join(outDir(), name), `${JSON.stringify(value, null, 1)}\n`);
}
