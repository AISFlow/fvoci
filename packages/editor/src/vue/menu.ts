/**
 * Tab leaves an editor menu (react/menu-keyboard.ts leaveMenu): the menu
 * closes and the opener takes focus back before the browser moves focus on,
 * so native Tab continues from the opener and a closed menu cannot trap it.
 * Vue applies the close on its next flush; the opener is focused now, so the
 * default Tab action already starts from it.
 */
export function leaveMenu(event: KeyboardEvent, opener: Element | null, close: () => void): void {
  event.stopPropagation();
  close();
  if (opener instanceof HTMLElement) opener.focus({ preventScroll: true });
}

/**
 * Put the editor's current ProseMirror selection back into the document's
 * native Selection without focusing the view. Toolbar buttons and popovers
 * take focus (Escape returns to the trigger) but Apply still needs the
 * highlighted text, and workspace-wiki-vue-controls asserts
 * `window.getSelection()` after the link dialog closes.
 */
export function restoreNativeSelection(editor: {
  isDestroyed: boolean;
  state: { selection: { from: number; to: number } };
  view: {
    dom: HTMLElement;
    domAtPos: (pos: number) => { node: Node; offset: number };
  };
}): void {
  if (editor.isDestroyed) return;
  const { from, to } = editor.state.selection;
  if (from === to) return;
  try {
    const start = editor.view.domAtPos(from);
    const end = editor.view.domAtPos(to);
    const range = editor.view.dom.ownerDocument.createRange();
    range.setStart(start.node, start.offset);
    range.setEnd(end.node, end.offset);
    const sel = editor.view.dom.ownerDocument.getSelection();
    if (!sel) return;
    // Adding a range to a focused contenteditable steals focus from the
    // toolbar trigger. Turning editing off for the add keeps the trigger
    // focused (workspace-wiki-vue-controls: Escape returns to 링크).
    const dom = editor.view.dom;
    const editable = dom.getAttribute("contenteditable");
    dom.contentEditable = "false";
    try {
      sel.removeAllRanges();
      sel.addRange(range);
    } finally {
      if (editable === null) dom.removeAttribute("contenteditable");
      else dom.setAttribute("contenteditable", editable);
    }
  } catch {
    /* A widget or unmapped pos cannot become a Range. */
  }
}
