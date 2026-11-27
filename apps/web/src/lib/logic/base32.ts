/** z-base-32, the alphabet of the app's identifiers (`wire.ts`). */

/** z-base-32: no characters that are easily confused when read aloud. */
const ID_ALPHABET = 'ybndrfg8ejkmcpqxot1uwisza345h769';

/** Base32 with the alphabet above; `pad` fills the last group with `=`. */
export function encodeBase32(data: Uint8Array, pad = false): string {
  let bits = 0;
  let value = 0;
  let output = '';
  for (const byte of data) {
    value = (value << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      output += ID_ALPHABET.charAt((value >>> (bits - 5)) & 31);
      bits -= 5;
    }
  }
  if (bits > 0) output += ID_ALPHABET.charAt((value << (5 - bits)) & 31);
  if (pad) while (output.length % 8 !== 0) output += '=';
  return output;
}
