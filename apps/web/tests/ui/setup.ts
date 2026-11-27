import '@testing-library/jest-dom/vitest';

// jsdom has no media queries. Every query answers "no": a light screen, a
// mouse. A test that needs another answer replaces `window.matchMedia`.
if (typeof window !== 'undefined' && typeof window.matchMedia !== 'function') {
  window.matchMedia = (query: string): MediaQueryList => {
    const list = new EventTarget() as MediaQueryList;
    Object.assign(list, {
      matches: false,
      media: query,
      onchange: null,
      addListener: () => undefined,
      removeListener: () => undefined,
    });
    return list;
  };
}
