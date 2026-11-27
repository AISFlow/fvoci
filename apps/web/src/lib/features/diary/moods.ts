/**
 * The faces of the moods of a day, each with its name: a mood is told by the
 * shape of its face and by its name, never by a color alone.
 */
import FaceAngryIcon from '@lucide/svelte/icons/face-angry';
import FaceGrinningIcon from '@lucide/svelte/icons/face-grinning';
import FaceNeutralIcon from '@lucide/svelte/icons/face-neutral';
import FaceSlightlyFrowningIcon from '@lucide/svelte/icons/face-slightly-frowning';
import FaceSlightlySmilingIcon from '@lucide/svelte/icons/face-slightly-smiling';
import type { Component } from 'svelte';

import { m } from '$lib/i18n/paraglide/messages.js';
import type { Mood } from '$lib/state/index.ts';

export interface MoodFace {
  icon: Component<{ class?: string }>;
  /** Its name in the reader's language. */
  name: () => string;
}

export const MOOD_FACES: Record<Mood, MoodFace> = {
  great: { icon: FaceGrinningIcon, name: m.diary_mood_great },
  good: { icon: FaceSlightlySmilingIcon, name: m.diary_mood_good },
  okay: { icon: FaceNeutralIcon, name: m.diary_mood_okay },
  bad: { icon: FaceSlightlyFrowningIcon, name: m.diary_mood_bad },
  awful: { icon: FaceAngryIcon, name: m.diary_mood_awful },
};
