/** Every account setting at its default, as a new account has them. */
import { DEFAULT_DISPLAY_SETTINGS, type DisplaySettings } from './display.ts';
import { DEFAULT_LANGUAGE_SETTINGS, type LanguageSettings } from './language.ts';
import { DEFAULT_NOTIFY_SETTINGS, type NotifySettings } from './notify-settings.ts';
import { DEFAULT_REGIONAL_SETTINGS, type RegionalSettings } from './regional.ts';

/** Every account setting: how things are shown, and what the reader is told of. */
export type AccountSettings = RegionalSettings &
  DisplaySettings &
  LanguageSettings &
  NotifySettings;

export const DEFAULT_ACCOUNT_SETTINGS: AccountSettings = {
  ...DEFAULT_REGIONAL_SETTINGS,
  ...DEFAULT_DISPLAY_SETTINGS,
  ...DEFAULT_LANGUAGE_SETTINGS,
  ...DEFAULT_NOTIFY_SETTINGS,
};

/** A member set to `null` goes back to its default. */
export type AccountSettingsPatch = { [K in keyof AccountSettings]?: AccountSettings[K] | null };
