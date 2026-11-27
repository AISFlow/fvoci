/** Earlier sidebar echoes must not replace a move that is still being written. */
import { expect, it, vi } from 'vitest';

import { loadTemplates, settle, setup } from './support.ts';

it('keeps a pending filing when the preceding write echoes, then reads the stored order', async () => {
  const { workspace, backend, world } = setup(await loadTemplates());
  await workspace.load();
  const [first, second] = workspace.sidebar;
  const board = first?.containers[0];
  if (!board || !second || !workspace.me) throw new Error('unexpected seed');
  const before = structuredClone(workspace.categories);
  let release: (() => void) | undefined;
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  const send = backend.moveContainerToCategory.bind(backend);
  vi.spyOn(backend, 'moveContainerToCategory').mockImplementation(async (...args) => {
    await held;
    await send(...args);
  });
  const moving = workspace.moveContainer(board.id, second.category.id, 0);
  world.tellUsers([workspace.me.id], { kind: 'categories', categories: before });
  await settle();
  try {
    expect(workspace.sidebar[0]?.containers.map((one) => one.id)).not.toContain(board.id);
    expect(workspace.sidebar[1]?.containers[0]?.id).toBe(board.id);
  } finally {
    release?.();
    await moving;
    await settle();
    workspace.close();
  }
});
