/** The switches of what the reader is told of, as the settings keep them. */
import { describe, expect, it } from 'vitest';

import { notifies, readNotifySettings } from '$lib/logic/notify-settings.ts';

describe('the switches of notifications', () => {
  it('are on unless stored off, whatever else is stored', () => {
    expect(readNotifySettings(undefined)).toEqual({
      notifyMentions: true,
      notifyAssignments: true,
      notifyComments: true,
      notifyDue: true,
    });
    expect(
      readNotifySettings({
        notifyMentions: false,
        notifyAssignments: 'no',
        notifyComments: 0,
        notifyDue: false,
      }),
    ).toEqual({
      notifyMentions: false,
      notifyAssignments: true,
      notifyComments: true,
      notifyDue: false,
    });
  });

  it('each tell of their own kind', () => {
    const settings = readNotifySettings({ notifyComments: false, notifyDue: false });
    expect(notifies(settings, 'comment')).toBe(false);
    expect(notifies(settings, 'due_soon')).toBe(false);
    expect(notifies(settings, 'mention')).toBe(true);
    expect(notifies(settings, 'assignment')).toBe(true);
  });
});
