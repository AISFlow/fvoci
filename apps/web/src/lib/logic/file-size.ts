/** How large a file is, as people read it. */

const BINARY_UNITS = ['KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB', 'ZiB', 'YiB'];
const DECIMAL_UNITS = ['kB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB'];

/** `1.5 MiB`; with `si`, powers of 1000 and `MB`. */
export function humanFileSize(bytes: number, si = false, decimals = 1): string {
  const step = si ? 1000 : 1024;
  if (Math.abs(bytes) < step) return `${bytes} B`;
  const units = si ? DECIMAL_UNITS : BINARY_UNITS;
  const factor = 10 ** decimals;
  let size = bytes;
  let unit = -1;
  do {
    size /= step;
    unit += 1;
  } while (Math.round(Math.abs(size) * factor) / factor >= step && unit < units.length - 1);
  return `${size.toFixed(decimals)} ${units[unit] ?? ''}`;
}
