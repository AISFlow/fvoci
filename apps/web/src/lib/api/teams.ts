/** Workspace groups, through the typed server routes. */
import type { ContainerTeam, Team, TeamRole, Teams } from '../state/team-types.ts';
import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';

export class ServerTeams implements Teams {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  onContainer(workspaceId: string, containerId: string): Promise<ContainerTeam[]> {
    return this.#http.request<Schemas['ContainerTeam'][]>(
      '/api/v1/w/{workspaceId}/containers/{containerId}/teams',
      { workspaceId, containerId },
    );
  }

  async assign(
    workspaceId: string,
    containerId: string,
    teamId: string,
    role: TeamRole,
  ): Promise<void> {
    const json: Schemas['ContainerTeamRole'] = { role };
    await this.#http.request(
      '/api/v1/w/{workspaceId}/containers/{containerId}/teams/{teamId}',
      { workspaceId, containerId, teamId },
      { method: 'PUT', json },
    );
  }

  async unassign(workspaceId: string, containerId: string, teamId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/w/{workspaceId}/containers/{containerId}/teams/{teamId}',
      { workspaceId, containerId, teamId },
      { method: 'DELETE' },
    );
  }

  list(workspaceId: string): Promise<Team[]> {
    return this.#http.request<Schemas['Team'][]>('/api/v1/w/{workspaceId}/teams', { workspaceId });
  }

  create(workspaceId: string, name: string): Promise<Team> {
    const json: Schemas['TeamName'] = { name };
    return this.#http.request<Schemas['Team']>(
      '/api/v1/w/{workspaceId}/teams',
      { workspaceId },
      { method: 'POST', json },
    );
  }

  rename(workspaceId: string, teamId: string, name: string): Promise<Team> {
    const json: Schemas['TeamName'] = { name };
    return this.#http.request<Schemas['Team']>(
      '/api/v1/w/{workspaceId}/teams/{teamId}',
      { workspaceId, teamId },
      { method: 'PUT', json },
    );
  }

  async delete(workspaceId: string, teamId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/w/{workspaceId}/teams/{teamId}',
      { workspaceId, teamId },
      { method: 'DELETE' },
    );
  }

  async addMember(workspaceId: string, teamId: string, userId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/w/{workspaceId}/teams/{teamId}/members/{userId}',
      { workspaceId, teamId, userId },
      { method: 'PUT' },
    );
  }

  async removeMember(workspaceId: string, teamId: string, userId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/w/{workspaceId}/teams/{teamId}/members/{userId}',
      { workspaceId, teamId, userId },
      { method: 'DELETE' },
    );
  }
}
