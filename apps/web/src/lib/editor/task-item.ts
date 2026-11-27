/**
 * A task of a body, whose box changes the task and nothing else.
 *
 * The library's task (`@tiptap/extension-list`) gives the editor the focus
 * when its box changes, and keeps a press of the mouse from the box itself
 * only, not from the label around it. On a touch screen the label is a
 * finger's square drawn over the box (editor.css), so a finger lands on the
 * label: the press focused the body there and put the selection in it, and
 * on a phone the focus of the body is its keyboard. Here a press anywhere on
 * the label leaves the focus and the selection where they were (none in the
 * body when it had none, the same caret when the reader was writing), and the
 * change is one step of the document, sent and undone as one. Space on a
 * focused box does the same and the box keeps the focus. A body that can
 * only be read keeps its tasks as they are, as before.
 *
 * The library's drawing of the task is kept (its box, its accessible name and
 * what an update redraws); only the press and the change are this file's.
 */
import { TaskItem } from '@tiptap/extension-list';
import { closeHistory } from '@tiptap/pm/history';
import { yUndoPluginKey } from '@tiptap/y-tiptap';

export const BodyTaskItem = TaskItem.extend({
  addNodeView() {
    const drawn = this.parent?.();
    if (!drawn) return null;
    const name = this.name;
    return (props) => {
      const view = drawn(props);
      const { editor, getPos } = props;
      const item = view.dom;
      const label = item.querySelector(':scope > label');
      const box = label?.querySelector('input[type="checkbox"]');
      if (!(label instanceof HTMLElement) || !(box instanceof HTMLInputElement)) return view;

      // Neither the browser (which would focus the body and put its selection
      // at the label) nor the editor (which leaves an event whose default is
      // prevented alone) takes a press on the label for one in the text.
      label.addEventListener('mousedown', (event) => {
        event.preventDefault();
      });
      // A press on the label passes to the box without the browser's focusing
      // it (WebKit does), which would take the focus from the text being
      // written. Where the body can only be read, the press does nothing: the
      // browser puts the box back itself, and no change is said.
      item.addEventListener(
        'click',
        (event) => {
          if (!(event.target instanceof Node) || !label.contains(event.target)) return;
          if (!editor.isEditable) event.preventDefault();
          else if (event.target !== box) {
            event.preventDefault();
            box.click();
          }
        },
        { capture: true },
      );

      // Taken on the way down, before the library's own listener on the box,
      // which would focus the editor: the event goes no further. The tick is
      // a step of undo of its own, not gathered with what was typed just
      // before or after it (a shared body gathers what comes close together).
      item.addEventListener(
        'change',
        (event) => {
          if (event.target !== box) return;
          event.stopPropagation();
          const at = editor.isEditable ? getPos() : undefined;
          const task = at === undefined ? null : editor.state.doc.nodeAt(at);
          if (at === undefined || task?.type.name !== name) {
            box.checked = !box.checked;
            return;
          }
          const undo = yUndoPluginKey.getState(editor.state)?.undoManager;
          undo?.stopCapturing();
          const tick = editor.state.tr.setNodeMarkup(at, undefined, {
            ...task.attrs,
            checked: box.checked,
          });
          editor.view.dispatch(closeHistory(tick));
          undo?.stopCapturing();
        },
        { capture: true },
      );
      // What is pressed or typed on the box is the box's: Enter, Tab or a
      // letter there is not one for the text at the editor's selection.
      return {
        ...view,
        stopEvent: (event: Event) => event.target instanceof Node && label.contains(event.target),
      };
    };
  },
});
