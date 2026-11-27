/**
 * The component around the read-only view: where the drawn body goes, what
 * stands in for a body that cannot be shown or is empty, and that it follows
 * its properties.
 */
import { render } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { describe, expect, it } from 'vitest';

import BodyView from '$lib/editor/BodyView.svelte';
import { renderBody } from '$lib/editor/render.ts';

import { doc, HAND_MADE, mention, paragraph, task, tasks, text } from '../support/body-docs.ts';

const invalid = createRawSnippet<[string[]]>((problems) => ({
  render: () => `<p role="alert" data-problems="${problems().length}">표시할 수 없는 본문</p>`,
}));
const empty = createRawSnippet(() => ({
  render: () => '<p data-testid="empty">내용 없음</p>',
}));

describe('BodyView', () => {
  it('puts the drawn body into an element that editor.css styles', () => {
    const body = HAND_MADE['text under several marks'];
    const { container } = render(BodyView, { doc: body, class: 'p-4' });
    const root = container.querySelector('.doc-body');
    expect(root?.tagName).toBe('DIV');
    expect(root?.classList.contains('p-4')).toBe(true);
    expect(container.querySelectorAll('.doc-body')).toHaveLength(1);

    const drawn = renderBody(body);
    const expected = document.createElement('div');
    if (drawn.ok) expected.append(drawn.fragment);
    expect(root?.children).toHaveLength(expected.children.length);
    expect(Array.from(root?.children ?? []).map((element) => element.outerHTML)).toEqual(
      Array.from(expected.children).map((element) => element.outerHTML),
    );
  });

  it('hands the callbacks on', () => {
    const { container } = render(BodyView, {
      doc: doc(paragraph(mention('u1')), tasks(task(true, paragraph(text('a'))))),
      resolveUser: (userId: string) => (userId === 'u1' ? '김하늘' : undefined),
      taskLabel: (label: string, checked: boolean) => `${checked ? 'done' : 'open'}: ${label}`,
    });
    expect(container.querySelector('.mention')?.textContent).toBe('@김하늘');
    const box = container.querySelector('input');
    expect(box?.getAttribute('aria-label')).toBe('done: a');
    expect(box?.checked).toBe(true);
    // The box is the one that was drawn, not a copy: it still refuses to change.
    box?.click();
    expect(box?.checked).toBe(true);
  });

  it('draws what the caller gives for a body that cannot be shown, and none of the body', () => {
    const { container } = render(BodyView, {
      doc: doc(paragraph(text('before')), { type: 'script', content: [text('alert(1)')] }),
      invalid,
      empty,
    });
    expect(container.querySelector('.doc-body')).toBeNull();
    expect(container.textContent).toBe('표시할 수 없는 본문');
    expect(container.querySelector('[role="alert"]')?.getAttribute('data-problems')).toBe('1');
  });

  it('draws nothing at all for a body that cannot be shown when the caller gives nothing', () => {
    const { container } = render(BodyView, { doc: { type: 'nonsense' } });
    expect(container.querySelector('.doc-body')).toBeNull();
    expect(container.textContent).toBe('');
  });

  it('draws what the caller gives for an empty body', () => {
    for (const nothing of [null, undefined, doc(paragraph())]) {
      const { container, unmount } = render(BodyView, { doc: nothing, invalid, empty });
      expect(container.querySelector('.doc-body')).toBeNull();
      expect(container.querySelector('[data-testid="empty"]')?.textContent).toBe('내용 없음');
      unmount();
    }
    const { container } = render(BodyView, { doc: null });
    expect(container.textContent).toBe('');
  });

  it('follows the document as it changes', async () => {
    const { container, rerender } = render(BodyView, {
      doc: doc(paragraph(text('one'))),
      invalid,
      empty,
    });
    expect(container.querySelector('.doc-body')?.textContent).toBe('one');
    const root = container.querySelector('.doc-body');

    await rerender({ doc: doc(paragraph(text('two')), paragraph(text('three'))) });
    expect(container.querySelector('.doc-body')).toBe(root);
    expect(Array.from(root?.children ?? []).map((element) => element.textContent)).toEqual([
      'two',
      'three',
    ]);

    await rerender({ doc: { type: 'doc', content: [{ type: 'table' }] } });
    expect(container.querySelector('.doc-body')).toBeNull();
    expect(container.querySelector('[role="alert"]')).not.toBeNull();

    await rerender({ doc: null });
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector('[data-testid="empty"]')).not.toBeNull();

    await rerender({ doc: doc(paragraph(text('back'))) });
    expect(container.querySelector('.doc-body')?.textContent).toBe('back');
    expect(container.querySelector('[data-testid="empty"]')).toBeNull();
  });

  it('follows the names of people as they become known', async () => {
    const body = doc(paragraph(mention('u1')));
    const { container, rerender } = render(BodyView, { doc: body });
    expect(container.querySelector('.mention')?.textContent).toBe('@u1');
    await rerender({ doc: body, resolveUser: () => 'Ada Lovelace' });
    expect(container.querySelector('.mention')?.textContent).toBe('@Ada Lovelace');
  });
});
