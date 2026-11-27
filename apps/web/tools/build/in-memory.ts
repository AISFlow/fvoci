/**
 * Marks a page of the app to run on the backend in memory: for the tools
 * that serve a build without the server (preview.ts, serve.ts), which have
 * no API. `src/app/backend.ts` reads the tag; the server writes none.
 */
export function inMemory(index: string): string {
  return index.replace(
    '<meta name="fvoci:base"',
    '<meta name="fvoci:backend" content="memory" />\n    <meta name="fvoci:base"',
  );
}
