/** Colour arithmetic for the token tests: OKLCH to sRGB, compositing, WCAG contrast. */

/** Linear-light sRGB, each channel 0..1, with an alpha. */
export interface Color {
  r: number;
  g: number;
  b: number;
  alpha: number;
}

const clamp = (value: number): number => Math.min(1, Math.max(0, value));

/** OKLCH (lightness 0..1, chroma, hue in degrees) to linear-light sRGB. */
export function fromOklch(lightness: number, chroma: number, hue: number, alpha = 1): Color {
  const a = chroma * Math.cos((hue * Math.PI) / 180);
  const b = chroma * Math.sin((hue * Math.PI) / 180);
  const l = (lightness + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (lightness - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (lightness - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return {
    r: clamp(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    g: clamp(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    b: clamp(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
    alpha,
  };
}

/** Parses `oklch(L C H)` and `oklch(L C H / A%)`, the only notation the tokens use. */
export function parseOklch(text: string): Color {
  const match = /^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)(?:\s*\/\s*([\d.]+)(%?))?\s*\)$/.exec(
    text.trim(),
  );
  if (!match) throw new Error(`not an oklch() colour: ${text}`);
  const [, lightness, chroma, hue, alpha, percent] = match;
  const opacity = alpha === undefined ? 1 : Number(alpha) / (percent === '%' ? 100 : 1);
  return fromOklch(Number(lightness), Number(chroma), Number(hue), opacity);
}

const encode = (linear: number): number =>
  linear <= 0.0031308 ? 12.92 * linear : 1.055 * linear ** (1 / 2.4) - 0.055;
const decode = (encoded: number): number =>
  encoded <= 0.04045 ? encoded / 12.92 : ((encoded + 0.055) / 1.055) ** 2.4;

/**
 * `top` painted over the opaque `bottom`, the way a browser does it: the
 * channels are mixed in gamma-encoded sRGB, not in linear light.
 */
export function over(top: Color, bottom: Color, opacity = 1): Color {
  const alpha = top.alpha * opacity;
  const mix = (t: number, b: number): number => decode(encode(t) * alpha + encode(b) * (1 - alpha));
  return { r: mix(top.r, bottom.r), g: mix(top.g, bottom.g), b: mix(top.b, bottom.b), alpha: 1 };
}

function luminance(color: Color): number {
  return 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
}

/** WCAG 2 contrast ratio of two opaque colours, 1..21. */
export function contrast(first: Color, second: Color): number {
  const [low, high] = [luminance(first), luminance(second)].sort((x, y) => x - y) as [
    number,
    number,
  ];
  return (high + 0.05) / (low + 0.05);
}
