/**
 * Where the logic reports that it met data it did not expect: an unknown
 * block type, a filter on a property that no longer exists. Such data is
 * handled, not thrown on; the report is for whoever develops the app.
 *
 * It is an object so that a test can watch `report` or silence it.
 */
export const diagnostics = {
  report(message: string): void {
    if (import.meta.env.DEV) console.error(`[fvoci] ${message}`);
  },
};
