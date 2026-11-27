/**
 * A person's name with their title and credentials, as the profile card and
 * the lists of members write it: "Dr. Younghwan Kim, PhD". Nowhere else: an
 * author, a mention, a cursor or a person on a card is the name alone.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { Credential, PersonName } from '$lib/logic/person-names.ts';

import { CREDENTIAL_LABELS, TITLE_LABELS } from './name-labels.ts';

/**
 * The international degrees and qualifications are written after a name by
 * their abbreviation in every language ("김영환 교수, PhD"); the screen's
 * words for them ("박사(PhD)") are for choosing one. The Korean ones have no
 * abbreviation and are written in the screen's words.
 */
const ABBREVIATIONS: Partial<Record<Credential, string>> = {
  phd: 'PhD',
  md: 'MD',
  dds: 'DDS',
  jd: 'JD',
  edd: 'EdD',
  mba: 'MBA',
  ms: 'MS',
  ma: 'MA',
  cpa: 'CPA',
  cfa: 'CFA',
  pe: 'PE',
  rn: 'RN',
};

/** How a credential is written after a name. */
export function credentialAfterName(credential: Credential): string {
  return ABBREVIATIONS[credential] ?? CREDENTIAL_LABELS[credential]();
}

/**
 * The name `shown` with the title where the screen's language puts it
 * (before it in English, "Dr. Kim"; after it in Korean, "김영환 박사") and
 * the credentials after a comma each, in the person's order. Without a
 * title and credentials, or without the parts (`name`), `shown` as it is.
 */
export function titledName(shown: string, name: PersonName | undefined): string {
  if (!name) return shown;
  const titled = name.title
    ? m.person_titled({ title: TITLE_LABELS[name.title](), name: shown })
    : shown;
  return [titled, ...name.credentials.map(credentialAfterName)].join(', ');
}
