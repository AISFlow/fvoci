/**
 * What the reader was told, from the server (`getNotifications` and the
 * routes beside it). They are the reader's own, of every workspace and
 * organization their session opens: no path is under a workspace.
 */
import type {
  Notification,
  Notifications,
  NotificationSubject,
} from '../state/notification-types.ts';
import type { Paging } from '../state/trash-reads.ts';

import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';

export class ServerNotifications implements Notifications {
  readonly #http: Http;

  constructor(http: Http) {
    this.#http = http;
  }

  async list(paging?: Paging): Promise<Notification[]> {
    const listed = await this.#http.request<Schemas['Notification'][]>(
      '/api/v1/notifications',
      {},
      { query: { page: paging?.page, perPage: paging?.perPage } },
    );
    return listed.map(notificationOf);
  }

  async unread(): Promise<number> {
    const unread = await this.#http.request<Schemas['UnreadNotifications']>(
      '/api/v1/notifications/unread',
    );
    return unread.count;
  }

  async read(notificationId: string): Promise<void> {
    await this.#http.request(
      '/api/v1/notifications/{notificationId}/read',
      { notificationId },
      { method: 'POST' },
    );
  }

  async readAll(through?: number): Promise<void> {
    await this.#http.request(
      '/api/v1/notifications/read',
      {},
      { method: 'POST', query: { through } },
    );
  }
}

function notificationOf(notification: Schemas['Notification']): Notification {
  const { subject } = notification;
  return {
    id: notification.id,
    createdAt: notification.createdAt,
    updatedAt: notification.updatedAt,
    readAt: notification.readAt,
    subject: subject ? subjectOf(subject) : null,
  };
}

function subjectOf(subject: Schemas['NotificationSubject']): NotificationSubject {
  return {
    kind: subject.kind,
    actorId: subject.actorId,
    times: subject.times,
    people: subject.people,
    workspaceId: subject.workspaceId,
    containerId: subject.containerId,
    containerKind: subject.containerKind,
    blockId: subject.blockId,
    blockKind: subject.blockKind,
    title: subject.title,
    commentId: subject.commentId,
    agentTokenName: subject.agentTokenName ?? undefined,
  };
}
