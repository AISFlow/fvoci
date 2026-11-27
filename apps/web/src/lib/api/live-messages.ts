/**
 * Decoding and routing JSON WebSocket events (`contracts/websocket.md`) for
 * `LiveChanges` (`live.ts`).
 */
import type { Container } from '../logic/wire.ts';
import type { Change } from '../state/backend.ts';
import type { BodySummary } from '../state/body-summary.ts';

import {
  attachmentOf,
  cardOfBlock,
  commentOf,
  containerOf,
  viewOf,
  type Schemas,
} from './convert.ts';
import { unitOfBlock } from './courses.ts';
import { entryOfBlock } from './diaries.ts';
import { pageOfBlock } from './wikis.ts';

type Members = Extract<Change, { kind: 'members' }>['members'];
export type ChangedBlock = Extract<Change, { kind: 'blocks' }>['blocks'][number];

/** What the messages do not carry, read over HTTP instead. */
export interface LiveReads {
  /** A board's or a wiki's members: a message carries one member, the screens want them all. */
  members(containerId: string): Promise<Members>;
  /** Records the workspace of a newly created/restored container heard on WS (`W1.2`). */
  learnContainer?(container: Container): void;
  /** Records a live view/comment block heard on WS so 1-round-trip patches stay current. */
  learnBlock?(block: ChangedBlock): void;
}

const CATEGORY_MESSAGE_TYPES = new Set([
  'categoryChanged',
  'containerCategoriesChanged',
  'categoriesReordered',
  'categoryContainersReordered',
]);

type Message = { workspaceId?: string } & (
  | { type: 'workspaceSubscribed' }
  | { type: 'blocksSubscribed' }
  | { type: 'workspaceUnsubscribed' }
  | { type: 'workspaceRoleChanged'; userId: string; role: string }
  | { type: 'containerChanged'; container: Schemas['Container'] }
  | { type: 'blockChanged'; block: Schemas['Block']; summary?: BodySummary }
  | { type: 'containerMemberChanged'; member: Schemas['ContainerMember'] }
  | {
      type: 'containerMemberRemoved';
      member: { containerId: string; userId: string };
    }
  | { type: 'userChanged'; userId: string }
  | { type: string }
);

/** Sink hooks used by `dispatchLiveMessage` to notify `LiveChanges` listeners. */
export interface LiveMessageSink {
  workspaceId(): string | undefined;
  readonly reads: LiveReads;
  resubscribeWorkspace(workspaceId: string | undefined): void;
  tellAll(change: Change): void;
  tellSelf(change: Change): void;
  tellContainer(containerId: string, change: Change, self: boolean): void;
  sidebarSoon(): void;
  readMembers(containerId: string): Promise<void>;
}

/** Routes a non-room, non-presence JSON WebSocket message to the appropriate listeners. */
export async function dispatchLiveMessage(
  parsed: Record<string, unknown>,
  sink: LiveMessageSink,
): Promise<void> {
  const message = parsed as Message;
  if (typeof message.type !== 'string') return;
  const activeWorkspace = sink.workspaceId();
  if (message.workspaceId !== undefined && message.workspaceId !== activeWorkspace) return;

  if (message.type === 'workspaceUnsubscribed') {
    sink.resubscribeWorkspace(activeWorkspace);
    sink.tellAll({ kind: 'resync' });
    return;
  }
  if (message.type === 'workspaceSubscribed' || message.type === 'blocksSubscribed') {
    sink.tellAll({ kind: 'resync' });
    return;
  }
  if (
    message.type === 'workspaceRoleChanged' &&
    'userId' in message &&
    typeof message.userId === 'string' &&
    'role' in message &&
    typeof message.role === 'string'
  ) {
    sink.sidebarSoon();
    sink.tellAll({
      kind: 'workspaceRole',
      workspaceId: message.workspaceId ?? activeWorkspace ?? '',
      userId: message.userId,
      role: message.role,
    });
    return;
  }
  if (message.type === 'notificationsChanged') {
    sink.tellSelf({ kind: 'notifications' });
  } else if (message.type === 'userChanged' && 'userId' in message) {
    sink.tellSelf({ kind: 'user', userId: message.userId });
  } else if (message.type === 'containerChanged' && 'container' in message) {
    const { container } = message;
    let change: Change;
    if (container.deletedAt === undefined) {
      const converted = containerOf(container);
      sink.reads.learnContainer?.(converted);
      change = { kind: 'container', container: converted };
    } else {
      change = { kind: 'containerDeleted', containerId: container.id };
    }
    sink.tellContainer(container.id, change, true);
  } else if (message.type === 'blockChanged' && 'block' in message) {
    receiveBlock(message.block, message.summary, sink);
  } else if (
    (message.type === 'containerMemberChanged' || message.type === 'containerMemberRemoved') &&
    'member' in message
  ) {
    sink.sidebarSoon();
    await sink.readMembers(message.member.containerId);
  } else if (CATEGORY_MESSAGE_TYPES.has(message.type)) {
    sink.sidebarSoon();
  }
}

function receiveBlock(
  block: Schemas['Block'],
  summary: BodySummary | undefined,
  sink: LiveMessageSink,
): void {
  if (block.deletedAt !== undefined) {
    sink.tellContainer(
      block.containerId,
      { kind: 'blocksDeleted', containerId: block.containerId, blockIds: [block.id] },
      false,
    );
    return;
  }
  if (block.type === 'page') {
    sink.tellContainer(
      block.containerId,
      { kind: 'pages', containerId: block.containerId, pages: [pageOfBlock(block)] },
      false,
    );
    return;
  }
  if (block.type === 'entry') {
    sink.tellContainer(
      block.containerId,
      { kind: 'entries', containerId: block.containerId, entries: [entryOfBlock(block)] },
      false,
    );
    return;
  }
  if (block.type === 'unit' || block.type === 'chapter') {
    sink.tellContainer(
      block.containerId,
      { kind: 'units', containerId: block.containerId, units: [unitOfBlock(block)] },
      false,
    );
    return;
  }
  const changed = changedBlock(block, summary);
  if (changed) {
    sink.reads.learnBlock?.(changed);
    sink.tellContainer(
      block.containerId,
      { kind: 'blocks', containerId: block.containerId, blocks: [changed] },
      false,
    );
  }
}

/**
 * A block of a message as the screens' changed block; other kinds are not
 * shown. A card's message carries the summary of its body in place of it.
 */
function changedBlock(
  block: Schemas['Block'],
  summary: BodySummary | undefined,
): ChangedBlock | undefined {
  switch (block.type) {
    case 'view':
      return viewOf(block);
    case 'card':
      return summary && cardOfBlock(block, summary);
    case 'comment':
      return commentOf(block);
    case 'attachment':
      return attachmentOf(block);
    default:
      return undefined;
  }
}
