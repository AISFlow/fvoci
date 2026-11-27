/**
 * What the reader is told of, as account settings: one switch for each kind
 * of notification (`notifyMentions`, `notifyAssignments`, `notifyComments`),
 * all on until the reader turns one off. What is off is not made at all:
 * turned on again, nothing of the time between comes.
 */

/** What a notification tells of. */
export type NotificationKind = 'mention' | 'assignment' | 'comment' | 'due_soon';

export interface NotifySettings {
  /** Somebody mentioned the reader. */
  notifyMentions: boolean;
  /** Somebody gave the reader a card. */
  notifyAssignments: boolean;
  /** Somebody commented on what the reader watches. */
  notifyComments: boolean;
  /** A card assigned to the reader is due today or soon. */
  notifyDue: boolean;
}

export const DEFAULT_NOTIFY_SETTINGS: NotifySettings = {
  notifyMentions: true,
  notifyAssignments: true,
  notifyComments: true,
  notifyDue: true,
};

/** The switch of each kind, in the order the settings show them. */
export const NOTIFY_SWITCHES: readonly { kind: NotificationKind; setting: keyof NotifySettings }[] =
  [
    { kind: 'mention', setting: 'notifyMentions' },
    { kind: 'assignment', setting: 'notifyAssignments' },
    { kind: 'comment', setting: 'notifyComments' },
    { kind: 'due_soon', setting: 'notifyDue' },
  ];

/**
 * The switches of the stored settings (`stored`, as the server or a store
 * keeps them): a member that is missing, or is no `true` or `false`, is on.
 */
export function readNotifySettings(stored: unknown): NotifySettings {
  const source = (typeof stored === 'object' && stored !== null ? stored : {}) as Record<
    string,
    unknown
  >;
  const on = (name: keyof NotifySettings): boolean =>
    typeof source[name] === 'boolean' ? source[name] : DEFAULT_NOTIFY_SETTINGS[name];
  return {
    notifyMentions: on('notifyMentions'),
    notifyAssignments: on('notifyAssignments'),
    notifyComments: on('notifyComments'),
    notifyDue: on('notifyDue'),
  };
}

/** Whether the reader is told of what is of `kind`. */
export function notifies(settings: NotifySettings, kind: NotificationKind): boolean {
  const found = NOTIFY_SWITCHES.find((one) => one.kind === kind);
  return found ? settings[found.setting] : true;
}
