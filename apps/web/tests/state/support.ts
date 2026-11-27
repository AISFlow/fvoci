/** A small world and the stores over it, for the tests of the state. */
import { afterEach } from 'vitest';

import { loadTemplates, seedWorld, type SeedTemplate } from '$lib/state/memory/seed.ts';
import { ServerBackend } from '$lib/api/backend.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import type { World } from '$lib/state/memory/world.ts';
import { Workspace, type BoardStore } from '$lib/state/index.ts';
import type { DocNode } from '$lib/editor/schema.ts';

export const STATUS = 'status';
export const OWNER = 'owner';

/** One board: a select property with three options, a person property, three cards, one view. */
export const SMALL: SeedTemplate = {
  key: 'small',
  onboarding: false,
  board: {
    id: 'b',
    title: 'Small',
    description: '',
    icon: '',
    showDescription: false,
    cardProperties: [
      {
        id: STATUS,
        name: 'Status',
        type: 'select',
        options: [
          { id: 'todo', label: 'To do', color: 'blue' },
          { id: 'doing', label: 'Doing', color: 'yellow' },
          { id: 'done', label: 'Done', color: 'green' },
        ],
      },
      { id: OWNER, name: 'Owner', type: 'person', options: [] },
    ],
  },
  views: [
    {
      id: 'v',
      title: 'Board',
      fields: {
        viewType: 'board',
        groupBy: STATUS,
        visibleGroups: ['todo', 'doing', 'done', ''],
        hiddenGroups: [],
        sort: [],
        cardOrder: ['c1', 'c2', 'c3'],
        filter: { operation: 'and', filters: [] },
      },
    },
  ],
  cards: [
    {
      id: 'c1',
      title: 'Alpha',
      fields: { icon: '', isTemplate: false, properties: { [STATUS]: 'todo' } },
      body: text('first body'),
    },
    {
      id: 'c2',
      title: 'Beta',
      fields: { icon: '', isTemplate: false, properties: { [STATUS]: 'todo' } },
      body: null,
    },
    {
      id: 'c3',
      title: 'Gamma',
      fields: { icon: '', isTemplate: false, properties: { [STATUS]: 'doing' } },
      body: null,
    },
  ],
  files: [],
};

export function text(value: string): DocNode {
  return {
    type: 'doc',
    content: [{ type: 'paragraph', content: [{ type: 'text', text: value }] }],
  };
}

export interface Setup {
  world: World;
  backend: MemoryBackend;
  /** The same world as another member of the boards. */
  other: MemoryBackend;
  workspace: Workspace;
}

/** Workspaces made by `setup` in the test running now, closed when it ends so no room or timer outlives it. */
const opened: Workspace[] = [];

afterEach(() => {
  for (const workspace of opened.splice(0)) workspace.close();
});

export function setup(templates: readonly SeedTemplate[] = [SMALL]): Setup {
  const { world, readerId } = seedWorld(templates);
  const backend = new MemoryBackend(world, readerId);
  const otherId = [...world.users.keys()].find((id) => id !== readerId);
  if (!otherId) throw new Error('the seed has one person only');
  const workspace = new Workspace({ backend, locale: () => 'en' });
  opened.push(workspace);
  return { world, backend, other: backend.as(otherId), workspace };
}

/** The workspace loaded and its first board open. */
export async function openFirstBoard(
  made: Setup = setup(),
): Promise<Setup & { board: BoardStore }> {
  await made.workspace.load();
  const first = made.workspace.sidebar.flatMap((entry) => entry.containers)[0];
  if (!first) throw new Error('the seed has no board');
  const board = await made.workspace.openBoard(first.id);
  return { ...made, board };
}

/** Lets the change messages of the world (sent in microtasks) arrive. */
export async function settle(): Promise<void> {
  for (let turn = 0; turn < 5; turn += 1) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
}

export { loadTemplates };

/** The workspace a server backend of the tests is in: the paths of its boards are under it. */
export const SERVER_WORKSPACE = 'w1';

/** A server backend under `basePath` that has entered `SERVER_WORKSPACE`. */
export function serverBackend(basePath: string): ServerBackend {
  const backend = new ServerBackend(basePath);
  backend.enter(SERVER_WORKSPACE);
  return backend;
}
