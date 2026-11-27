/**
 * The words of a code block's copy button (code-copy.ts), in the reader's
 * language. Apart from the button: what draws a body that is only shown
 * (code-view.ts) is fetched when the body has code, and is given these by
 * the component that shows the body (BodyView.svelte), so that it brings
 * neither messages nor anything of the framework with it.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { shortcutLabel } from '$lib/keys.ts';

import type { CopyTexts } from './code-copy.ts';

export const CODE_COPY_TEXTS: CopyTexts = {
  copy: () => m.editor_code_copy(),
  copied: () => m.editor_code_copied(),
  failed: () => m.editor_code_copy_failed({ shortcut: shortcutLabel('C') }),
};
