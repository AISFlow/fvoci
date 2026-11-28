/**
 * Container, membership, permissions, cross-workspace copy, template, and
 * public-link HTTP operations for `ServerBackend` (`api/backend.ts`), including
 * per-container `BlockTurns` serialization and `learnPlaces` workspace tracking.
 */
import type { BoardView, Container, ContainerMember, ContainerPatch } from '../logic/wire.ts';
import {
  BackendError,
  type BoardDraft,
  type ContainerPermissions,
  type ContainerRole,
  type CopiedContainerOutput,
  type CopyToWorkspaceInput,
  type PublicLink,
  type PublicLinkChange,
  type ViewDraft,
} from '../state/backend.ts';
import type { BlockTurns } from '../state/write.ts';

import {
  containerOf,
  containerPatchOf,
  memberOf,
  newBoardOf,
  publicLinkOf,
  viewOf,
  type Schemas,
} from './convert.ts';
import type { Http, RequestOptions } from './http.ts';
import { underWorkspace, type WorkspacePath } from './requests.ts';
import { makeWelcomeBoard, templatesOf } from './templates.ts';

export interface ContainerRequestDeps {
  readonly http: Http;
  readonly turns: BlockTurns;
  inWorkspace<T>(
    path: WorkspacePath,
    params?: Record<string, string>,
    options?: RequestOptions,
    workspaceId?: string,
  ): Promise<T>;
  learnPlaces(containers: readonly Container[]): void;
  learnView(view: BoardView): void;
}

export class ServerContainers {
  readonly #deps: ContainerRequestDeps;

  constructor(deps: ContainerRequestDeps) {
    this.#deps = deps;
  }

  async createBoard(
    workspaceId: string,
    draft: BoardDraft,
    firstView: ViewDraft,
  ): Promise<{ board: Container; view: BoardView }> {
    const rawBoard = await this.#deps.inWorkspace<Schemas['Container']>(
      '/boards',
      {},
      { method: 'POST', json: newBoardOf(draft, firstView) },
      workspaceId,
    );
    const board = containerOf(rawBoard);
    this.#deps.learnPlaces([board]);
    const [rawView] = await this.#deps.inWorkspace<Schemas['Block'][]>(
      '/boards/{boardId}/views',
      { boardId: board.id },
      {},
      workspaceId,
    );
    if (!rawView) throw new BackendError(500, { detail: 'the server made no view' });
    const view = viewOf(rawView);
    this.#deps.learnView(view);
    return { board, view };
  }

  reorderCardProperties(boardId: string, propertyIds: string[]): Promise<string[]> {
    return this.#deps.turns.run(`container ${boardId}`, () =>
      this.#deps.inWorkspace<string[]>(
        '/boards/{boardId}/property-order',
        { boardId },
        { method: 'PUT', json: propertyIds },
      ),
    );
  }

  async patchContainer(id: string, patch: ContainerPatch): Promise<Container> {
    const request = containerPatchOf(patch);
    const send = async (): Promise<Container> => {
      const updated = containerOf(
        await this.#deps.inWorkspace<Schemas['Container']>(
          '/containers/{containerId}',
          { containerId: id },
          { method: 'PATCH', json: request },
        ),
      );
      this.#deps.learnPlaces([updated]);
      return updated;
    };
    const mutatesProperties =
      (patch.updatedCardProperties?.length ?? 0) > 0 ||
      (patch.deletedCardProperties?.length ?? 0) > 0 ||
      Object.keys(patch.updatedProperties ?? {}).length > 0 ||
      (patch.deletedProperties?.length ?? 0) > 0 ||
      patch.dueDatePropertyId !== undefined ||
      patch.doneWhen !== undefined;
    if (mutatesProperties) {
      return this.#deps.turns.run(`container ${id}`, send);
    }
    return send();
  }

  myPermissions(containerId: string): Promise<ContainerPermissions> {
    return this.#deps.inWorkspace<Schemas['ContainerPermissions']>(
      '/containers/{containerId}/my-permissions',
      { containerId },
    );
  }

  async copyContainerToWorkspace(
    containerId: string,
    request: CopyToWorkspaceInput,
  ): Promise<CopiedContainerOutput> {
    const json: Schemas['CopyContainerToWorkspaceRequest'] = {
      targetWorkspaceId: request.targetWorkspaceId,
      ...(request.title === undefined ? {} : { title: request.title }),
    };
    const result = await this.#deps.inWorkspace<Schemas['CopiedContainerResult']>(
      '/containers/{containerId}/copy-to-workspace',
      { containerId },
      { method: 'POST', json },
    );
    const container = containerOf(result.container);
    this.#deps.learnPlaces([container]);
    return { container, missingFilesCount: result.missingFilesCount };
  }

  async searchBoards(workspaceId: string, query: string): Promise<Container[]> {
    const boards = await this.#deps.http.request<Schemas['Container'][]>(
      underWorkspace('/boards/search'),
      { workspaceId },
      { query: { q: query } },
    );
    const found = boards.map(containerOf);
    this.#deps.learnPlaces(found);
    return found;
  }

  async templates(workspaceId: string): Promise<Container[]> {
    const found = await templatesOf(this.#deps.http, workspaceId);
    this.#deps.learnPlaces(found);
    return found;
  }

  async duplicateContainer(id: string, asTemplate: boolean, to: string): Promise<Container> {
    const made = await this.#deps.inWorkspace<Schemas['ContainersAndBlocks']>(
      '/containers/{containerId}/duplicate',
      { containerId: id },
      { method: 'POST', query: { asTemplate: String(asTemplate), toWorkspace: to } },
    );
    const [rawBoard] = made.containers;
    if (!rawBoard) throw new BackendError(500, { detail: 'the server made no copy' });
    const board = containerOf(rawBoard);
    this.#deps.learnPlaces([board]);
    return board;
  }

  onboard(workspaceId: string): Promise<string> {
    return makeWelcomeBoard(this.#deps.http, workspaceId);
  }

  async deleteContainer(id: string): Promise<void> {
    await this.#deps.inWorkspace(
      '/containers/{containerId}',
      { containerId: id },
      { method: 'DELETE' },
    );
  }

  async restoreContainer(id: string): Promise<Container> {
    const restored = containerOf(
      await this.#deps.inWorkspace<Schemas['Container']>(
        '/containers/{containerId}/restore',
        { containerId: id },
        { method: 'POST' },
      ),
    );
    this.#deps.learnPlaces([restored]);
    return restored;
  }

  async members(id: string): Promise<ContainerMember[]> {
    const members = await this.#deps.inWorkspace<Schemas['ContainerMember'][]>(
      '/containers/{containerId}/members',
      { containerId: id },
    );
    return members.map(memberOf);
  }

  async addMember(id: string, userId: string, role: ContainerRole): Promise<ContainerMember> {
    const request: Schemas['NewMember'] = { userId, role };
    return memberOf(
      await this.#deps.inWorkspace<Schemas['ContainerMember']>(
        '/containers/{containerId}/members',
        { containerId: id },
        { method: 'POST', json: request },
      ),
    );
  }

  async setMemberRole(id: string, userId: string, role: ContainerRole): Promise<ContainerMember> {
    const patch: Schemas['ContainerMemberPatch'] = { role };
    return memberOf(
      await this.#deps.inWorkspace<Schemas['ContainerMember']>(
        '/containers/{containerId}/members/{userId}',
        { containerId: id, userId },
        { method: 'PUT', json: patch },
      ),
    );
  }

  async removeMember(id: string, userId: string): Promise<void> {
    await this.#deps.inWorkspace(
      '/containers/{containerId}/members/{userId}',
      { containerId: id, userId },
      { method: 'DELETE' },
    );
  }

  async publicLink(id: string): Promise<PublicLink> {
    return publicLinkOf(
      await this.#deps.inWorkspace<Schemas['PublicLink']>('/containers/{containerId}/public-link', {
        containerId: id,
      }),
    );
  }

  async setPublicLink(id: string, change: PublicLinkChange): Promise<PublicLink> {
    const json: Schemas['PublicLinkRequest'] = {
      enabled: change.enabled,
      ...(change.regenerate ? { regenerate: true } : {}),
    };
    return publicLinkOf(
      await this.#deps.inWorkspace<Schemas['PublicLink']>(
        '/containers/{containerId}/public-link',
        { containerId: id },
        { method: 'POST', json },
      ),
    );
  }

  async joinContainer(id: string): Promise<ContainerMember> {
    return memberOf(
      await this.#deps.inWorkspace<Schemas['ContainerMember']>(
        '/containers/{containerId}/join',
        { containerId: id },
        { method: 'POST' },
      ),
    );
  }
}
