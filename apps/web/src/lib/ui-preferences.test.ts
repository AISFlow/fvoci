import assert from "node:assert/strict";
import test from "node:test";
import { applyTextScale, readThemePreference, setThemePreference, startUiPreferences } from "./ui-preferences";

function fixture() {
  const descriptors = new Map(["window", "document", "localStorage"].map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  const storage = new Map<string, string>();
  const classes = new Set<string>();
  const style = { fontSize: "" };
  let dark = false;
  const mediaListeners = new Set<() => void>();
  const storageListeners = new Set<(event: { key: string | null }) => void>();
  const media = {
    get matches() { return dark; },
    addEventListener: (_: string, listener: () => void) => mediaListeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => mediaListeners.delete(listener),
  };
  Object.defineProperties(globalThis, {
    window: { configurable: true, value: {
      matchMedia: () => media,
      addEventListener: (_: string, listener: (event: { key: string | null }) => void) => storageListeners.add(listener),
      removeEventListener: (_: string, listener: (event: { key: string | null }) => void) => storageListeners.delete(listener),
    } },
    document: { configurable: true, value: { documentElement: { style, classList: {
      toggle: (key: string, enabled: boolean) => enabled ? classes.add(key) : classes.delete(key),
    } } } },
    localStorage: { configurable: true, value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, value),
    } },
  });
  return { storage, classes, style, mediaListeners, storageListeners,
    system: (next: boolean) => { dark = next; mediaListeners.forEach((listener) => listener()); },
    dispose: () => descriptors.forEach((descriptor, key) => {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }),
  };
}

test("UI preferences restore valid settings, follow system theme, and release listeners", () => {
  const f = fixture();
  try {
    f.storage.set("fvoci-text-scale", "20");
    const stop = startUiPreferences();
    assert.equal(f.style.fontSize, "20px");
    f.system(true);
    assert.equal(f.classes.has("dark"), true);
    setThemePreference("light");
    f.system(true);
    assert.equal(f.classes.has("dark"), false);
    f.storage.set("fvoci-theme", "dark");
    f.storageListeners.forEach((listener) => listener({ key: "fvoci-theme" }));
    assert.equal(f.classes.has("dark"), true);
    stop();
    assert.equal(f.mediaListeners.size, 0);
    assert.equal(f.storageListeners.size, 0);
  } finally { f.dispose(); }
});

test("UI preferences reject unsupported scales and tolerate denied browser storage", () => {
  const f = fixture();
  try {
    applyTextScale(18);
    for (const scale of [0, 17, Number.NaN, 1000]) applyTextScale(scale);
    assert.equal(f.style.fontSize, "18px");
    assert.equal(f.storage.get("fvoci-text-scale"), "18");
    f.storage.set("fvoci-theme", "foreign-value");
    assert.equal(readThemePreference(), "system");
    Object.defineProperty(globalThis, "localStorage", { configurable: true, get: () => { throw new Error("denied"); } });
    assert.equal(readThemePreference(), "system");
    assert.doesNotThrow(() => setThemePreference("dark"));
    assert.equal(f.classes.has("dark"), true);
    assert.doesNotThrow(() => applyTextScale(16));
    assert.equal(f.style.fontSize, "16px");
  } finally { f.dispose(); }
});
