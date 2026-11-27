/**
 * The server's shapes (`contract.ts`, made from `contracts/openapi.json`)
 * and the shapes the screens work with (`lib/state/backend.ts`,
 * `lib/logic/wire.ts`), one into the other.
 *
 * Where the two differ the screens' shape stays and the difference is
 * settled here.
 */
import type { CardPreview } from '../editor/card-preview.ts';
import type { DocNode } from '../editor/schema.ts';
import { readDisplaySettings } from '../logic/display.ts';
import { readLanguageSettings } from '../logic/language.ts';
import { readNotifySettings } from '../logic/notify-settings.ts';
import type { PersonName } from '../logic/person-names.ts';
import { readRegionalSettings } from '../logic/regional.ts';
import {
  createBoardView,
  createCard,
  type Block,
  type Container,
  type ContainerMember,
  type ContainerPatch,
  type BoardView,
} from '../logic/wire.ts';
import {
  BackendError,
  refusedMember,
  type AccountSettings,
  type BoardDraft,
  type AccountSettingsPatch,
  type Attachment,
  type CardDetail,
  type CardDraft,
  type CardPatch,
  type CardSummary,
  type CardTelling,
  type Comment,
  type CommentVersion,
  type Category,
  type PublicLink,
  type WorkspaceInfo,
  type User,
  type ViewDraft,
} from '../state/backend.ts';
import type { BodySummary } from '../state/body-summary.ts';

import type { components } from './contract.ts';

export type Schemas = components['schemas'];

/** The parts of a name, its choice, title and credentials, of whatever carries them. */
export function personNameOf(name: Schemas['PersonName']): PersonName {
  return {
    firstName: name.firstName,
    lastName: name.lastName,
    latinFirstName: name.latinFirstName,
    latinLastName: name.latinLastName,
    customName: name.customName,
    nameShown: name.nameShown,
    title: name.title,
    credentials: name.credentials,
  };
}

/**
 * A user: a name to log in with, the one to be shown by and the parts it is
 * made of, and no email address (it is never answered).
 */
export function userOf(user: Schemas['User']): User {
  return {
    id: user.id,
    username: user.username,
    displayName: user.displayName,
    name: personNameOf(user),
    email: '',
    isAdmin: user.isAdmin,
    createdAt: user.createdAt,
    updatedAt: user.updatedAt,
    ...(user.deletedAt === undefined ? {} : { deletedAt: user.deletedAt }),
  };
}

/** A workspace the reader is in, or a guest of. */
export function workspaceOf(workspace: Schemas['UserWorkspace']): WorkspaceInfo {
  return {
    id: workspace.id,
    title: workspace.name,
    role: workspace.role,
    orgId: workspace.orgId,
    kind: workspace.kind,
    ssoRequired: workspace.ssoRequired,
    passwordRequired: workspace.passwordRequired,
  };
}

/** The workspaces the reader is in, those they are a guest of too, in the server's order. */
export function workspacesOf(workspaces: Schemas['UserWorkspace'][]): WorkspaceInfo[] {
  return workspaces.map(workspaceOf);
}

function doneWhenOf(raw: Schemas['Container']['doneWhen']): Container['doneWhen'] {
  if (raw === undefined) return undefined;
  if (raw === null) return null;
  if (raw.optionIds && raw.optionIds.length > 0) {
    return { kind: 'option', propertyId: raw.propertyId, optionIds: raw.optionIds };
  }
  return { kind: 'checkbox', propertyId: raw.propertyId };
}

function wireDoneWhen(rule: ContainerPatch['doneWhen']): Schemas['ContainerPatch']['doneWhen'] {
  if (rule === undefined) return undefined;
  if (rule === null) return null;
  if (rule.kind === 'option') {
    return { propertyId: rule.propertyId, optionIds: rule.optionIds };
  }
  return { propertyId: rule.propertyId, optionIds: [] };
}

export function containerOf(board: Schemas['Container']): Container {
  const doneWhen = doneWhenOf(board.doneWhen);
  return {
    id: board.id,
    workspaceId: board.workspaceId,
    createdBy: board.createdBy,
    modifiedBy: board.modifiedBy,
    visibility: board.visibility,
    kind: board.kind,
    minimumRole: board.minimumRole,
    title: board.title,
    description: board.description,
    icon: board.icon,
    showDescription: board.showDescription,
    isTemplate: board.isTemplate,
    isBuiltin: board.isBuiltin,
    templateVersion: board.templateVersion,
    properties: board.properties as Container['properties'],
    cardProperties: board.cardProperties,
    ...(board.dueDatePropertyId === undefined
      ? {}
      : { dueDatePropertyId: board.dueDatePropertyId }),
    ...(doneWhen === undefined ? {} : { doneWhen }),
    createdAt: board.createdAt,
    updatedAt: board.updatedAt,
    ...(board.deletedAt === undefined ? {} : { deletedAt: board.deletedAt }),
  };
}

/** A membership. An inherited role stays separate from the direct `role`. */
export function memberOf(member: Schemas['ContainerMember']): ContainerMember {
  return {
    containerId: member.containerId,
    userId: member.userId,
    role: member.role,
    synthetic: member.synthetic ?? false,
    inheritedRole: member.inheritedRole ?? undefined,
  };
}

/** A block of the API as the screens' block. Its body, if any, is the card's or comment's to carry. */
export function blockOf(block: Schemas['Block']): Block {
  const { body: _body, number, ...rest } = block;
  const numbered = number === undefined || number === null ? {} : { number };
  return { ...rest, ...numbered, fields: { ...block.fields } };
}

/** The body of a block or a card, as the screens' document. */
export function bodyOf(body: Schemas['Block']['body']): DocNode | null {
  return (body ?? null) as DocNode | null;
}

export function viewOf(block: Schemas['Block']): BoardView {
  return createBoardView(blockOf(block));
}

/**
 * The card summary of a card block and what is said of its body: the
 * `summary` beside the block of a change message, whose block has no body
 * (`contracts/websocket.md` 3), or that of the body of a block read whole
 * (`summaryOfBody`). It has no `lastCommentAt`: the board keeps the one it had.
 */
export function cardOfBlock(block: Schemas['Block'], summary: BodySummary): CardSummary {
  return { ...createCard(blockOf(block)), ...summary };
}

/** A card of the cards API: its members are where a card block has its `fields`. */
export function cardOf(card: Schemas['Card']): CardSummary {
  return {
    ...createCard({
      id: card.id,
      containerId: card.boardId,
      parentId: card.parentId,
      createdBy: card.createdBy,
      modifiedBy: card.modifiedBy,
      type: 'card',
      title: card.title,
      fields: { icon: card.icon, isTemplate: card.isTemplate, properties: card.properties },
      createdAt: card.createdAt,
      updatedAt: card.updatedAt,
      ...(card.deletedAt === undefined ? {} : { deletedAt: card.deletedAt }),
      ...(card.number === null ? {} : { number: card.number }),
    }),
    hasBody: card.hasBody,
    tasksTotal: card.tasksTotal,
    tasksDone: card.tasksDone,
    lastCommentAt: card.lastCommentAt,
  };
}

/** The previews of a card list asked for them, by card. */
export function previewsOf(cards: readonly Schemas['Card'][]): Map<string, CardPreview> {
  return new Map(
    cards.flatMap((card) =>
      card.preview
        ? [[card.id, { imageFileId: card.preview.imageFileId, text: card.preview.text }]]
        : [],
    ),
  );
}

/** A card read alone, with its body (a list of cards carries none). */
export function cardDetailOf(card: Schemas['Card']): CardDetail {
  return { ...cardOf(card), body: bodyOf(card.body) };
}

/** A document as the contract's body: the contract leaves its members open. */
export function wireBody(doc: DocNode): Schemas['Body'] {
  return doc as unknown as Schemas['Body'];
}

/**
 * The request that makes a card of a draft (`createCard`): a quiet one
 * (`quiet`, a copy or a card made again) tells nobody whom it names or is
 * given to (`disableNotify`); its writer watches it all the same.
 */
export function newCardRequest(draft: CardDraft): {
  method: 'POST';
  json: Schemas['NewCard'];
  query: { disableNotify: 'true' | undefined };
} {
  const json: Schemas['NewCard'] = {
    ...(draft.parentId ? { parentId: draft.parentId } : {}),
    title: draft.title ?? '',
    icon: draft.icon ?? '',
    isTemplate: draft.isTemplate ?? false,
    properties: draft.properties ?? {},
    ...(draft.body ? { body: wireBody(draft.body) } : {}),
  };
  return { method: 'POST', json, query: { disableNotify: draft.quiet ? 'true' : undefined } };
}

/**
 * The request that changes a card (`patchCard`): a quiet one (`telling`,
 * undo or redo) tells nobody whom it gives the card to (`disableNotify`).
 */
export function patchCardRequest(
  patch: CardPatch,
  telling?: CardTelling,
): {
  method: 'PATCH';
  json: Schemas['CardPatch'];
  query: { disableNotify: 'true' | undefined };
} {
  const json: Schemas['CardPatch'] = {};
  if (patch.parentId !== undefined) json.parentId = patch.parentId;
  if (patch.title !== undefined) json.title = patch.title;
  if (patch.icon !== undefined) json.icon = patch.icon;
  const updated = patch.updatedProperties ?? {};
  if (Object.keys(updated).length > 0) json.updatedProperties = updated;
  const deleted = patch.deletedProperties ?? [];
  if (deleted.length > 0) json.deletedProperties = deleted;
  return { method: 'PATCH', json, query: { disableNotify: telling?.quiet ? 'true' : undefined } };
}

/**
 * A comment: a block whose text is its body. A comment that has no body and
 * keeps its text in the title, as comments were first stored, is shown with
 * each line of the title as a paragraph of plain text.
 */
export function commentOf(block: Schemas['Block']): Comment {
  const body = bodyOf(block.body);
  const lines = block.title.trim().split(/\r?\n/);
  return {
    ...blockOf(block),
    type: 'comment',
    body:
      body ??
      (lines.join('') === ''
        ? null
        : {
            type: 'doc',
            content: lines.map((text) =>
              text
                ? { type: 'paragraph', content: [{ type: 'text', text }] }
                : { type: 'paragraph' },
            ),
          }),
  };
}

/** An earlier text of a comment. */
export function commentVersionOf(version: Schemas['CommentVersion']): CommentVersion {
  return { ...version, body: bodyOf(version.body) };
}

/**
 * An attachment block. Its file is `fields.fileId` (older blocks name it
 * `attachmentId`, which the server takes too) and its name the title. Its
 * size and type are in its fields when this app made it (`createAttachment`);
 * a block another client made may have neither: they are 0 and empty, and
 * the screen leaves an unknown size out.
 */
export function attachmentOf(block: Schemas['Block']): Attachment {
  const fields = block.fields;
  const file = fields.fileId ?? fields.attachmentId;
  return {
    ...blockOf(block),
    type: 'attachment',
    fileId: typeof file === 'string' ? file : '',
    name: block.title,
    size: typeof fields.size === 'number' ? fields.size : 0,
    contentType: typeof fields.contentType === 'string' ? fields.contentType : '',
  };
}

/**
 * A category. The contract lists the boards of a category with a `hidden`
 * flag each; the screens keep the shown and the hidden apart.
 */
export function categoryOf(category: Schemas['CategoryContainers']): Category {
  const metadata = category.containerMetadata;
  return {
    id: category.id,
    name: category.name,
    userId: category.userId,
    workspaceId: category.workspaceId,
    type: category.type,
    collapsed: category.collapsed,
    sortOrder: category.sortOrder,
    containerIds: metadata.map((entry) => entry.containerId),
    hiddenContainerIds: metadata.filter((entry) => entry.hidden).map((entry) => entry.containerId),
    createdAt: category.createdAt,
    updatedAt: category.updatedAt,
  };
}

/** The settings, under the names of the contract. */
export function settingsOf(settings: Schemas['UserSettings']): AccountSettings {
  return {
    ...readRegionalSettings(settings),
    ...readDisplaySettings(settings),
    ...readLanguageSettings(settings),
    ...readNotifySettings(settings),
  };
}

export function settingsChangeOf(patch: AccountSettingsPatch): Schemas['SettingsChange'] {
  return patch;
}

/** A public link; one that was never on has no token. */
export function publicLinkOf(link: Schemas['PublicLink']): PublicLink {
  return { enabled: link.enabled, token: link.token ?? '' };
}

/**
 * A board and its first view, to be made in one request (`createContainersAndBlocks`);
 * the workspace is the one of the request's path.
 */
export function newBoardAndViewOf(
  draft: BoardDraft,
  firstView: ViewDraft,
): Schemas['NewContainersAndBlocks'] {
  const boardRef = 'new-board';
  return {
    containers: [
      {
        id: boardRef,
        visibility: draft.visibility ?? 'private',
        title: draft.title,
        description: draft.description ?? '',
        icon: draft.icon ?? '',
        cardProperties: draft.cardProperties ?? [],
      },
    ],
    blocks: [
      {
        id: 'new-view',
        containerId: boardRef,
        type: 'view',
        title: firstView.title,
        fields: firstView.fields,
      },
    ],
  };
}

/**
 * A change of a board, as `patchContainer` takes it. A minimum role the
 * contract does not have (none, or admin) is refused here as the server
 * would refuse it.
 */
export function containerPatchOf(patch: ContainerPatch): Schemas['ContainerPatch'] {
  const { minimumRole, updatedCardProperties, doneWhen, ...rest } = patch;
  if (minimumRole === '' || minimumRole === 'admin') {
    throw new BackendError(400, {
      detail: 'no such minimum role',
      errors: [refusedMember('minimumRole', 'no such minimum role')],
    });
  }
  const wiredDoneWhen = wireDoneWhen(doneWhen);
  return {
    ...rest,
    ...(minimumRole === undefined ? {} : { minimumRole }),
    ...(updatedCardProperties === undefined ? {} : { updatedCardProperties }),
    ...(wiredDoneWhen === undefined ? {} : { doneWhen: wiredDoneWhen }),
  };
}
