/**
 * Edits that the browser makes to a body on its own, which the editor's
 * blocks either refuse or let through. Each is written once here, with the
 * browser it is for, and every block that needs it takes it from here.
 */
import type { EditorView, ViewMutationRecord } from '@tiptap/pm/view';

/**
 * A handler of `beforeinput` that refuses every edit while the focus is on
 * one of a block's controls (`selector`: a code block's language and copy
 * buttons, a file block's name and download). The edit comes to the editor
 * itself, and WebKit would make it where the caret was in the text.
 */
export function noTypingFromControls(
  selector: string,
): (view: EditorView, event: Event) => boolean {
  return (view, event) => {
    const focused = view.dom.ownerDocument.activeElement;
    if (!(focused instanceof Element) || !focused.matches(selector)) return false;
    if (!view.dom.contains(focused)) return false;
    event.preventDefault();
    return true;
  };
}

/**
 * Whether a change of the DOM is the one iOS makes when it splits a block's
 * `<pre>` on its own Enter: a change of the children of an element that
 * holds the block's text (`contentDOM`). A node view's `ignoreMutation` lets
 * it reach the editor, whose Enter then puts the document right.
 */
export function contentWasSplit(mutation: ViewMutationRecord, contentDOM: Node): boolean {
  return mutation.type === 'childList' && mutation.target.contains(contentDOM);
}
