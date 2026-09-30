import { chromium } from "@playwright/test";
import { join } from "node:path";
import { evidence, focusNative, keys, nativeImeEvents, observe, snapshot } from "./native";
const profile = join(evidence, "chrome-profile");
const context = await chromium.launchPersistentContext(profile, {
  headless: false,
  viewport: null,
  env: { ...process.env, TMPDIR: "." },
  args: ["--ozone-platform=x11", "--no-first-run", "--no-default-browser-check"],
});
try {
  const page = context.pages()[0];
  if (!page) throw new Error("persistent native browser has no initial page");
  await page.goto(
    'data:text/html,<title>FVOCI native IME smoke</title><style>textarea{font:32px sans-serif;width:800px;height:200px;margin:40px}</style><textarea aria-label="Owned native IME smoke"></textarea>',
  );
  await observe(page);
  const field = page.locator("textarea");
  await field.waitFor({ state: "visible" });
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  );
  await focusNative(page, field, profile);
  keys("Shift+space");
  keys("g");
  await page.waitForTimeout(300);
  await snapshot(page, "first-jamo");
  keys("k", "s");
  await page.waitForTimeout(300);
  await snapshot(page, "preedit-han");
  keys("r", "m", "f");
  await page.waitForTimeout(300);
  await snapshot(page, "preedit-hangul");
  keys("space");
  await page.waitForTimeout(300);
  await snapshot(page, "commit");
  const value = await field.inputValue();
  const events = await nativeImeEvents(page);
  if (
    value !== "한글 " ||
    !events.some((e) => e.type === "compositionupdate") ||
    !events.some((e) => e.type === "compositionend")
  )
    throw new Error("Native IME smoke failed: " + JSON.stringify({ value, events }));
  console.log(
    JSON.stringify({
      result: "PASS native IBus Hangul smoke",
      value,
      compositionEvents: events.filter((e) => e.type.startsWith("composition")),
    }),
  );
} finally {
  await context.close();
}
