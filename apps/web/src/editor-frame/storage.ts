/**
 * A `Storage` kept in memory, for the editor's settings (main.ts): the frame
 * has no origin, so it has no `localStorage`, and what the editor remembers
 * (its theme, the toolbars shown) lasts as long as the frame does.
 */
export function memoryStorage(initial: Readonly<Record<string, string>> = {}): Storage {
  const items = new Map(Object.entries(initial));
  return {
    get length() {
      return items.size;
    },
    clear() {
      items.clear();
    },
    getItem: (key) => items.get(key) ?? null,
    key: (index) => [...items.keys()][index] ?? null,
    removeItem(key) {
      items.delete(key);
    },
    setItem(key, value) {
      items.set(key, value);
    },
  };
}
