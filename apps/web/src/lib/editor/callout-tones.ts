/**
 * The tones of a callout as a reader meets them: each one's name, and the
 * tone a callout is drawn in. Shared by the editor (callout.ts) and the
 * view that only shows a body (render-blocks.ts), which loads no editor.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

import { DEFAULT_TONE, isCalloutTone, type CalloutTone } from './schema-rules.ts';

/** The name of a tone, said and shown. */
export function toneName(tone: CalloutTone): string {
  switch (tone) {
    case 'info':
      return m.editor_callout_info();
    case 'tip':
      return m.editor_callout_tip();
    case 'warn':
      return m.editor_callout_warn();
    case 'danger':
      return m.editor_callout_danger();
  }
}

/** The tone a callout is drawn in: its own, or a note's where its own is not on the list. */
export const toneOf = (value: unknown): CalloutTone =>
  isCalloutTone(value) ? value : DEFAULT_TONE;
