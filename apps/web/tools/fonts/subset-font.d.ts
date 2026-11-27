/** The part of `subset-font` this tool uses. The package ships no types. */
declare module 'subset-font' {
  interface SubsetOptions {
    targetFormat?: 'sfnt' | 'woff' | 'woff2';
    /** Axis tag to a fixed value, or to a reduced range. */
    variationAxes?: Readonly<
      Record<string, number | { min?: number; max?: number; default?: number }>
    >;
    noHinting?: boolean;
  }

  export default function subsetFont(
    font: Buffer,
    text: string,
    options?: SubsetOptions,
  ): Promise<Buffer>;
}
