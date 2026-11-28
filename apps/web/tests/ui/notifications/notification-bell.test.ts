/**
 * The bell (features/notifications/NotificationBell.svelte) in a simulated
 * page: the count on it, and when "Mark all as read" may be pressed: only
 * once the list is read and while one of it is unread. Pressed, it marks
 * what is shown, and what came before it, as read. One an agent did says
 * whose agent did it.
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import NotificationBell from '$lib/features/notifications/NotificationBell.svelte';
import { NotificationInbox } from '$lib/state/notification-inbox.svelte.ts';
import type { Notification, Notifications } from '$lib/state/notification-types.ts';

afterEach(() => {
  cleanup();
});

const told = (id: string, updatedAt: number, readAt = 0): Notification => ({
  id,
  createdAt: updatedAt,
  updatedAt,
  readAt,
  archivedAt: 0,
  subject: {
    kind: 'mention',
    actorId: '',
    times: 1,
    people: 1,
    workspaceId: 'w',
    containerId: 'b',
    containerKind: 'board',
    blockId: `card-${id}`,
    blockKind: 'card',
    title: `Card ${id}`,
    commentId: '',
  },
});

/** A bell over mutable notifications whose `list` resolves each call when `answerNext` is called. */
function mount(initial: Notification[]) {
  let items = initial.map((one) => ({ ...one }));
  const pending: (() => void)[] = [];
  const notifications = {
    list: vi.fn<Notifications['list']>(
      () =>
        new Promise<Notification[]>((resolve) => {
          pending.push(() => {
            resolve(items.map((one) => ({ ...one })));
          });
        }),
    ),
    unread: vi.fn<Notifications['unread']>(() =>
      Promise.resolve(items.filter((one) => one.readAt === 0).length),
    ),
    read: vi.fn<Notifications['read']>((id: string) => {
      items = items.map((one) => (one.id === id && one.readAt === 0 ? { ...one, readAt: 1 } : one));
      return Promise.resolve();
    }),
    readAll: vi.fn<Notifications['readAll']>((through?: number) => {
      const bound = through ?? Number.POSITIVE_INFINITY;
      items = items.map((one) =>
        one.readAt === 0 && one.updatedAt <= bound ? { ...one, readAt: bound } : one,
      );
      return Promise.resolve();
    }),
    markUnread: vi.fn<Notifications['markUnread']>(() => Promise.resolve()),
    archive: vi.fn<Notifications['archive']>(() => Promise.resolve()),
    unarchive: vi.fn<Notifications['unarchive']>(() => Promise.resolve()),
    archiveRead: vi.fn<Notifications['archiveRead']>(() => Promise.resolve()),
  } satisfies Notifications;
  const inbox = new NotificationInbox(
    notifications,
    { watchSelf: () => () => undefined, usersById: () => Promise.resolve([]) },
    () => undefined,
  );
  render(NotificationBell, {
    props: {
      inbox,
      locale: 'en-US',
      hrefOf: () => '/somewhere',
      onGo: vi.fn(),
      sourceOf: () => 'Roadmap · Example org',
    },
  });
  const answerNext = (): void => {
    const next = pending.shift();
    if (!next) throw new Error('no pending notifications.list() call');
    next();
  };
  return { notifications, inbox, answerNext, pendingCount: () => pending.length };
}

const readAll = (): HTMLElement => screen.getByTestId('read-all-notifications');

describe('the bell', () => {
  it('shows how many are unread, and nothing when none is', async () => {
    const { inbox } = mount([told('a', 2), told('b', 1, 5)]);
    expect(screen.queryByTestId('unread-notifications')).toBeNull();
    await inbox.refresh();
    expect(await screen.findByTestId('unread-notifications')).toHaveTextContent('1');
    inbox.unread = 0;
    await waitFor(() => {
      expect(screen.queryByTestId('unread-notifications')).toBeNull();
    });
  });

  it('lets all be marked as read only once the list is read and while one of it is unread', async () => {
    const { notifications, answerNext, pendingCount } = mount([
      told('a', 30),
      told('b', 20, 25),
      told('c', 10),
    ]);
    await fireEvent.click(screen.getByTestId('open-notifications'));
    expect(await screen.findByTestId('notifications')).toBeInTheDocument();
    expect(readAll()).toBeDisabled();

    answerNext();
    await waitFor(() => {
      expect(readAll()).toBeEnabled();
    });
    await fireEvent.click(readAll());
    expect(notifications.readAll).toHaveBeenCalledWith(30);
    // Settle the follow-up refresh list call and confirm the button stays disabled.
    await waitFor(() => {
      expect(pendingCount()).toBe(1);
    });
    answerNext();
    await waitFor(() => {
      expect(readAll()).toBeDisabled();
      expect(notifications.list).toHaveBeenCalledTimes(2);
    });
  });

  it('reads all with ⇧U only where its button would: while one listed is unread', async () => {
    const { notifications, answerNext } = mount([told('a', 30, 35), told('b', 20, 25)]);
    await fireEvent.click(screen.getByTestId('open-notifications'));
    answerNext();
    const line = await waitFor(() => {
      const found = document.querySelector<HTMLElement>('[data-notification] [data-row-focus]');
      if (!found) throw new Error('no line to press the key on');
      return found;
    });
    expect(readAll()).toBeDisabled();
    await fireEvent.keyDown(line, { key: 'U', code: 'KeyU', shiftKey: true });
    expect(notifications.readAll).not.toHaveBeenCalled();
  });

  it('disables mark all while reloading a previously opened list with unread items', async () => {
    const { answerNext, pendingCount } = mount([told('a', 30), told('b', 10)]);
    const trigger = screen.getByTestId('open-notifications');
    await fireEvent.click(trigger);
    expect(await screen.findByTestId('notifications')).toBeInTheDocument();
    answerNext();
    await waitFor(() => {
      expect(readAll()).toBeEnabled();
    });

    // Close and reopen: stale unread items remain in memory while the fresh fetch is in flight.
    await fireEvent.click(trigger);
    await waitFor(() => {
      expect(screen.queryByTestId('notifications')).toBeNull();
    });
    await fireEvent.click(trigger);
    expect(await screen.findByTestId('notifications')).toBeInTheDocument();
    expect(pendingCount()).toBe(1);
    expect(readAll()).toBeDisabled();

    answerNext();
    await waitFor(() => {
      expect(readAll()).toBeEnabled();
    });
  });

  it('lets nothing be marked of a list that is all read, or empty', async () => {
    for (const items of [[told('a', 2, 3)], []]) {
      const { answerNext } = mount(items);
      await fireEvent.click(screen.getByTestId('open-notifications'));
      expect(await screen.findByTestId('notifications')).toBeInTheDocument();
      answerNext();
      await waitFor(() => {
        expect(
          screen.queryAllByRole('listitem').length +
            screen.queryAllByTestId('no-notifications').length,
        ).toBeGreaterThan(0);
      });
      expect(readAll()).toBeDisabled();
      cleanup();
    }
  });

  it('names the token of an agent that did it, and nothing of one a person did', async () => {
    const byAgent = told('a', 2);
    if (byAgent.subject) byAgent.subject.agentTokenName = 'Planner';
    const { answerNext } = mount([byAgent, told('b', 1)]);
    await fireEvent.click(screen.getByTestId('open-notifications'));
    answerNext();
    const agents = await screen.findAllByTestId('notification-agent');
    expect(agents.map((one) => one.textContent.trim())).toEqual(['Agent · Planner']);
  });
});
