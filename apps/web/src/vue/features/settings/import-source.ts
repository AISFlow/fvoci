export type ImportSource = "markdown-zip" | "office-file" | "notion-zip";

/** The formats the server converts (source list plus Markdown/text). */
export const IMPORT_ACCEPT: Record<ImportSource, string> = {
  "markdown-zip": ".zip,application/zip",
  "office-file": ".pdf,.docx,.pptx,.xlsx,.odt,.odp,.ods,.hwp,.hwpx,.md,.markdown,.txt",
  "notion-zip": ".zip,application/zip",
};

export const IMPORT_NO_PROJECT = "none";

export const IMPORT_SOURCES: readonly ImportSource[] = ["markdown-zip", "office-file", "notion-zip"];
