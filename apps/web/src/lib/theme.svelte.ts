/**
 * Light or dark appearance of the app.
 *
 * `preference` is what the person chose, `resolved` is what is on screen: the
 * choice itself, or the system setting while the choice is `system`. The
 * tokens in styles/app.css switch on the `dark` class of the root element,
 * which `applyTheme` keeps in step.
 *
 * Remembering the choice between visits belongs to the settings module and is
 * not done here.
 */

export type ThemePreference = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

const DARK_QUERY = '(prefers-color-scheme: dark)';

function systemTheme(): ResolvedTheme {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return 'light';
  return window.matchMedia(DARK_QUERY).matches ? 'dark' : 'light';
}

class ThemeState {
  preference = $state<ThemePreference>('system');
  #system = $state<ResolvedTheme>(systemTheme());

  /** The appearance that is actually shown. */
  get resolved(): ResolvedTheme {
    return this.preference === 'system' ? this.#system : this.preference;
  }

  /** Re-reads the system setting. Called when the media query changes. */
  refreshSystem(): void {
    this.#system = systemTheme();
  }
}

export const theme = new ThemeState();

/** Sets the choice and updates the root element. */
export function applyTheme(preference: ThemePreference): void {
  theme.preference = preference;
  syncRoot();
}

function syncRoot(): void {
  if (typeof document === 'undefined') return;
  document.documentElement.classList.toggle('dark', theme.resolved === 'dark');
}

/**
 * Starts following the system setting and applies the current choice.
 * Returns a function that stops following.
 */
export function watchSystemTheme(): () => void {
  syncRoot();
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') {
    return () => undefined;
  }
  const query = window.matchMedia(DARK_QUERY);
  const onChange = (): void => {
    theme.refreshSystem();
    syncRoot();
  };
  query.addEventListener('change', onChange);
  return () => {
    query.removeEventListener('change', onChange);
  };
}
