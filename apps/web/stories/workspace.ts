/** A small, public world using the same memory backend as the application's tests. */
import { DEFAULT_ACCOUNT_SETTINGS } from '$lib/logic/account-settings.ts';
import { createBoardView, relationLink, type Container, type BoardView } from '$lib/logic/wire.ts';
import { getLocale } from '$lib/i18n/paraglide/runtime.js';
import { Workspace } from '$lib/state/index.ts';
import { MemoryBackend, memberOf } from '$lib/state/memory/backend.ts';
import { seedUsers } from '$lib/state/memory/users.ts';
import { World } from '$lib/state/memory/world.ts';

import { exampleBlock, exampleCard, STORY_NOW } from './fixtures.ts';

export interface WorkspaceExample {
  count?: number;
  kind?: BoardView['fields']['viewType'];
  readOnly?: boolean;
  managed?: boolean;
  cycle?: boolean;
  late?: boolean;
  variedRelations?: boolean;
}

export function exampleWorkspace({
  count = 8,
  kind = 'calendar',
  readOnly = false,
  managed = false,
  cycle = false,
  late = false,
  variedRelations = false,
}: WorkspaceExample = {}) {
  const world = new World(() => STORY_NOW);
  world.workspace = {
    id: 'example-workspace',
    title: 'Example workspace · 예시 작업 공간',
    role: 'admin',
  };
  // Members of the example workspace, as the server has them: a board takes only its workspace's people.
  seedUsers(
    world,
    ['example-reader', 'example-member', 'example-guest'].map((id, index) => ({
      id,
      username: id,
      displayName: `Example member ${index + 1} · 예시 구성원`,
      email: `${id}@example.test`,
      createdAt: STORY_NOW,
      updatedAt: STORY_NOW,
    })),
  );
  world.settings.set('example-reader', {
    ...DEFAULT_ACCOUNT_SETTINGS,
    timeZone: 'Asia/Seoul',
    weekStart: 'monday',
    hourCycle: 'h23',
  });
  const board: Container = {
    id: 'example-board',
    workspaceId: world.workspace.id,
    createdBy: 'example-reader',
    modifiedBy: 'example-reader',
    visibility: 'private',
    minimumRole: 'editor',
    title: 'Example board with a long name · 이름이 긴 예시 보드',
    description: '',
    showDescription: false,
    isTemplate: false,
    templateVersion: 0,
    properties: {},
    cardProperties: [
      { id: 'due', name: 'Due · 날짜', type: 'date', options: [] },
      { id: 'before', name: 'Depends on · 선행 카드', type: 'relation', options: [] },
      {
        id: 'status',
        name: 'Status · 상태',
        type: 'select',
        options: [
          { id: 'todo', label: 'To do · 할 일', color: 'blue' },
          { id: 'done', label: 'Done · 완료', color: 'green' },
        ],
      },
    ],
    createdAt: STORY_NOW,
    updatedAt: STORY_NOW,
  };
  world.containers.set(board.id, board);
  world.members.set(board.id, [
    ...(managed ? [] : [memberOf(board.id, 'example-reader', readOnly ? 'viewer' : 'admin')]),
    memberOf(board.id, 'example-member', 'admin'),
  ]);
  const view = createBoardView({
    ...exampleBlock('example-view', 'view'),
    title: 'Example view',
    fields: {
      viewType: kind,
      dateProperty: 'due',
      dependencyPropertyId: 'before',
      ganttZoom: 'day',
      groupBy: 'status',
      shownProperties: ['status', 'due'],
      visibleGroups: ['todo', 'done', ''],
      hiddenGroups: [],
      cardOrder: Array.from({ length: count }, (_, index) => `example-card-${index + 1}`),
    },
  });
  world.blocks.set(view.id, view);
  seedCards(world, count, cycle, late, variedRelations);
  const backend = new MemoryBackend(world, 'example-reader');
  const workspace = new Workspace({ backend, locale: getLocale });
  return { world, backend, workspace };
}

export async function loadWorkspace(example: ReturnType<typeof exampleWorkspace>): Promise<void> {
  await example.workspace.load();
  await example.workspace.openBoard('example-board');
}

const VARIED_LINKS = [
  ['fs', 1],
  ['ss', 2],
  ['ff', -1],
  ['sf', 1],
] as const;

function seedCards(
  world: World,
  count: number,
  cycle: boolean,
  late: boolean,
  variedRelations: boolean,
): void {
  let previousDated: string | undefined;
  let variedIndex = 0;
  for (let index = 1; index <= count; index += 1) {
    const card = exampleCard(index);
    if (index === 1) card.title += ' — Long example / 긴 예시'.repeat(5);
    card.fields.properties.status = index % 3 === 0 ? 'done' : 'todo';
    const dated = index % 5 !== 0;
    if (dated) {
      const day = late && index === 2 ? 5 : 4 + ((index - 1) % 8) * 3;
      card.fields.properties.due = {
        from: `2026-11-${String(day).padStart(2, '0')}`,
        to: `2026-11-${String(day + 2).padStart(2, '0')}`,
      };
    }
    if (variedRelations) {
      if (dated && previousDated !== undefined) {
        const [kind, lagDays] = VARIED_LINKS[variedIndex % VARIED_LINKS.length] ?? ['fs', 0];
        card.fields.properties.before = [relationLink(previousDated, kind, lagDays)];
        variedIndex += 1;
      }
    } else if (index > 1) {
      card.fields.properties.before = [relationLink(`example-card-${index - 1}`)];
    }
    if (dated) previousDated = card.id;
    if (cycle && index === 1 && count >= 3) {
      card.fields.properties.before = [relationLink('example-card-3')];
    }
    world.blocks.set(card.id, card);
  }
}
