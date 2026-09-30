export type ThemePreference = "system" | "light" | "dark";

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function store(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* Keep the applied session preference. */
  }
}

export function readThemePreference(): ThemePreference {
  const value = read("fvoci-theme");
  return value === "dark" || value === "light" ? value : "system";
}

function applyTheme(preference: ThemePreference): void {
  const dark =
    preference === "dark" ||
    (preference === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.classList.toggle("dark", dark);
}

export function setThemePreference(preference: ThemePreference): void {
  store("fvoci-theme", preference);
  applyTheme(preference);
}

export function applyTextScale(scale: number): void {
  if (scale !== 16 && scale !== 18 && scale !== 20) return;
  document.documentElement.style.fontSize = `${String(scale)}px`;
  store("fvoci-text-scale", String(scale));
}

/** Call once at app startup; the returned function releases this app's listeners. */
export function startUiPreferences(): () => void {
  let preference = readThemePreference();
  applyTheme(preference);
  applyTextScale(Number(read("fvoci-text-scale") ?? 16));
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const systemChanged = () => {
    preference = readThemePreference();
    if (preference === "system") applyTheme(preference);
  };
  const stored = (event: StorageEvent) => {
    if (event.key === "fvoci-theme" || event.key === null) {
      preference = readThemePreference();
      applyTheme(preference);
    }
    if (event.key === "fvoci-text-scale" || event.key === null)
      applyTextScale(Number(read("fvoci-text-scale") ?? 16));
  };
  media.addEventListener("change", systemChanged);
  window.addEventListener("storage", stored);
  return () => {
    media.removeEventListener("change", systemChanged);
    window.removeEventListener("storage", stored);
  };
}
