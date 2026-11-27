/**
 * The reader's choice of the numbers of the lines of code blocks
 * (code-lines.ts), as state for the switch in the settings.
 */
import {
  CODE_LINES_ATTRIBUTE,
  CODE_LINES_STORAGE_KEY,
  rememberedCodeLines,
  showCodeLines,
} from './code-lines.ts';

class CodeLines {
  shown = $state(rememberedCodeLines());

  set(shown: boolean): void {
    this.shown = shown;
    showCodeLines(shown);
  }
}

export const codeLines = new CodeLines();

/**
 * Puts the choice remembered on the device on the page, as the app starts,
 * and again whenever another tab of the device changes it (or its storage is
 * cleared): followed here, not remembered again.
 */
export function startCodeLines(page: Document = document): void {
  const put = (): void => {
    page.documentElement.toggleAttribute(CODE_LINES_ATTRIBUTE, codeLines.shown);
  };
  put();
  page.defaultView?.addEventListener('storage', (event) => {
    if (event.key !== null && event.key !== CODE_LINES_STORAGE_KEY) return;
    codeLines.shown = rememberedCodeLines();
    put();
  });
}
