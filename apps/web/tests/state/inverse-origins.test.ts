/** Equal values from different writes must keep their different inverses. */
import { expect, it } from 'vitest';

import type { BlockPatch } from '$lib/logic/wire.ts';
import { Inverses } from '$lib/state/inverses.ts';

function held() {
  let refuse = (): void => undefined;
  const promise = new Promise<void>((_resolve, reject) => {
    refuse = () => {
      reject(new Error('refused'));
    };
  });
  return { promise, refuse };
}

it.each(['title', 'field', 'deleted'])(
  'keeps a later successful source of the same %s value',
  async (kind) => {
    const inverses = new Inverses<BlockPatch>('updatedFields', 'deletedFields');
    const patch = (value: string): BlockPatch =>
      kind === 'title'
        ? { title: value }
        : kind === 'deleted' && value === 'A'
          ? { deletedFields: ['key'] }
          : { updatedFields: { key: value } };
    const gate = held();
    const first = inverses
      .write('v', patch('A'), patch('start'), () => gate.promise)
      .catch(() => undefined);
    const fromFirst = patch('A');
    inverses.keep('v', fromFirst);
    await inverses.write('v', patch('B'), fromFirst, async () => undefined);
    const fromSecond = patch('B');
    inverses.keep('v', fromSecond);
    await inverses.write('v', patch('A'), fromSecond, async () => undefined);
    const fromThird = patch('A');
    inverses.keep('v', fromThird);
    gate.refuse();
    await first;
    const value = (one: BlockPatch): unknown =>
      kind === 'title' ? one.title : one.updatedFields?.key;
    expect(value(fromFirst)).toBe('start');
    expect(value(fromSecond)).toBe('B');
    expect(fromThird).toEqual(patch('A'));
  },
);

it.each([false, true])(
  'follows dependent failures in either response order (first earlier: %s)',
  async (earlier) => {
    const inverses = new Inverses<BlockPatch>('updatedFields', 'deletedFields');
    const gates = [held(), held()] as const;
    const first = inverses
      .write('v', { title: 'A' }, { title: 'start' }, () => gates[0].promise)
      .catch(() => undefined);
    const fromFirst = { title: 'A' };
    inverses.keep('v', fromFirst);
    const second = inverses
      .write('v', { title: 'B' }, fromFirst, () => gates[1].promise)
      .catch(() => undefined);
    const fromSecond = { title: 'B' };
    inverses.keep('v', fromSecond);
    const writes = [first, second];
    for (const at of earlier ? [0, 1] : [1, 0]) {
      gates[at]?.refuse();
      await writes[at];
    }
    expect(fromSecond.title).toBe('start');
  },
);

it('a successful equal-valued write is an independent source too', async () => {
  const inverses = new Inverses<BlockPatch>('updatedFields', 'deletedFields');
  const gate = held();
  const first = inverses
    .write(
      'v',
      { updatedFields: { x: ['A'] } },
      { updatedFields: { x: ['start'] } },
      () => gate.promise,
    )
    .catch(() => undefined);
  const repeated = { updatedFields: { x: ['A'] } };
  inverses.keep('v', repeated);
  await inverses.write('v', { updatedFields: { x: ['A'] } }, repeated, async () => undefined);
  const later = { updatedFields: { x: ['A'] } };
  inverses.keep('v', later);
  gate.refuse();
  await first;
  expect(later.updatedFields.x).toEqual(['A']);
});
