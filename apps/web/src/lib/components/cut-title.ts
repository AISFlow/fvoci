/**
 * The whole of a name that is shown cut short (`truncate`), as its tooltip:
 * only while it is cut. A tooltip of the very words shown tells the eye
 * nothing, and some screen readers read it after the name, twice over.
 *
 * Use: `{@attach cutTitle(label)}` on the element the pointer rests on; the
 * text measured is its `.truncate` part, or the element itself. It is
 * measured as the pointer comes over it, before a tooltip shows (a width
 * that changed meanwhile, the window's or the name's, is measured then),
 * and taken away as it leaves and with the label it was for: a reader who
 * comes by the keyboard, after another name is shown, is not read an old one.
 */
import type { Attachment } from 'svelte/attachments';
import { on } from 'svelte/events';

export function cutTitle(label: string): Attachment<HTMLElement> {
  return (element) => {
    const untitled = (): void => {
      element.removeAttribute('title');
    };
    const stops = [
      on(element, 'pointerenter', () => {
        const text = element.querySelector<HTMLElement>('.truncate') ?? element;
        if (text.scrollWidth > text.clientWidth) element.title = label;
        else untitled();
      }),
      on(element, 'pointerleave', untitled),
    ];
    return () => {
      for (const stop of stops) stop();
      untitled();
    };
  };
}
