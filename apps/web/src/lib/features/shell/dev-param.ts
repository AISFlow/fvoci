/**
 * What the query of an address says about the layout readout: `?dev=1` (or
 * any value but `0` and `false`) turns it on, `?dev=0` turns it off, and an
 * address without the parameter says nothing.
 */
export function devParam(search: string): boolean | undefined {
  const value = new URLSearchParams(search).get('dev');
  if (value === null) return undefined;
  return value !== '0' && value !== 'false';
}
