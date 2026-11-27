/**
 * A key is read from where the caret is.
 *
 * The editor learns that the caret moved without it (a key it leaves to the
 * browser, as End and Home are; a press in the text) from the page's
 * `selectionchange`, an event the browser sends a moment later. A browser
 * under load answers input first, so the next key can come before that
 * event: the editor would read it from where the caret was, and an arrow
 * would pass a block instead of selecting it, Enter would break the line
 * where the caret had been. So before any key the editor is sent that event
 * now, and reads the page's selection as it does then; where it has already
 * heard, it finds nothing changed.
 */
import { Extension } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';

import { isComposing } from '$lib/keys.ts';

export const KeyReadsCaret = Extension.create({
  name: 'keyReadsCaret',
  // Before every other reader of keys (the editing place is 1000).
  priority: 10_000,

  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey('keyReadsCaret'),
        props: {
          handleDOMEvents: {
            keydown: (view, event) => {
              // While an input method composes, the text and its caret are the method's.
              if (!isComposing(event) && view.hasFocus()) {
                view.dom.ownerDocument.dispatchEvent(new Event('selectionchange'));
              }
              return false;
            },
          },
        },
      }),
    ];
  },
});
