/**
 * The address of a link written in a field (LinkField.svelte): read as a link
 * takes it, and put on the chosen text or taken off it.
 */
import type { Editor } from '@tiptap/core';
import { TextSelection } from '@tiptap/pm/state';

import { isAllowedHref } from './schema.ts';

/** A scheme before the address: `https:`, `mailto:`. */
const SCHEME = /^[a-z][a-z0-9+.-]*:/i;
/** A host and its port, which a scheme looks like: `localhost:3000`, `example.com:8080/x`. */
const HOST_AND_PORT = /^(?:localhost|[^\s/:@]+\.[^\s/:@]+):\d{1,5}(?:[/?#]|$)/i;
/** An address of mail. */
const MAIL = /^[^\s/@]+@[^\s/@]+$/;

/** The address as a link takes it: one without a scheme is a web address, or mail. */
export function withScheme(typed: string): string {
  if (SCHEME.test(typed) && !HOST_AND_PORT.test(typed)) return typed;
  return MAIL.test(typed) ? `mailto:${typed}` : `https://${typed}`;
}

/** The address of the link the selection is in, or nothing. */
export function linkAt(editor: Editor): string {
  const href: unknown = editor.getAttributes('link').href;
  return typeof href === 'string' ? href : '';
}

/** Whether a link can be put on where the selection is (chosen text, not code), or taken off. */
export function canLink(editor: Editor): boolean {
  if (editor.isActive('link')) return true;
  const { selection } = editor.state;
  if (!(selection instanceof TextSelection) || selection.empty) return false;
  return !selection.$from.parent.type.spec.code && !selection.$to.parent.type.spec.code;
}

/**
 * Puts the address typed on the chosen text as a link, or takes the link off
 * when it is empty, and gives the editor the focus; false, and nothing done,
 * when the address may not be a link.
 */
export function putLink(editor: Editor, typed: string): boolean {
  const address = typed.trim();
  const chain = editor.chain().focus().extendMarkRange('link');
  if (address === '') {
    chain.unsetLink().run();
    return true;
  }
  const href = withScheme(address);
  if (!isAllowedHref(href)) return false;
  chain.setLink({ href }).run();
  return true;
}
