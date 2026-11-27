/**
 * The words of the screen's language for the codes of a name: the choices,
 * the titles and the credentials. Every code of the contract's lists has its
 * words (the types say so); the server stores the code.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { Credential, NameShown, Title } from '$lib/logic/person-names.ts';

export const NAME_SHOWN_LABELS: Record<NameShown, () => string> = {
  name: m.account_name_shown_name,
  latin_name: m.account_name_shown_latin_name,
  custom_name: m.account_name_shown_custom_name,
  username: m.account_name_shown_username,
};

export const TITLE_LABELS: Record<Title, () => string> = {
  dr: m.person_title_dr,
  prof: m.person_title_prof,
};

export const CREDENTIAL_LABELS: Record<Credential, () => string> = {
  phd: m.person_credential_phd,
  md: m.person_credential_md,
  dds: m.person_credential_dds,
  jd: m.person_credential_jd,
  edd: m.person_credential_edd,
  mba: m.person_credential_mba,
  ms: m.person_credential_ms,
  ma: m.person_credential_ma,
  cpa: m.person_credential_cpa,
  cfa: m.person_credential_cfa,
  pe: m.person_credential_pe,
  rn: m.person_credential_rn,
  kr_attorney: m.person_credential_kr_attorney,
  kr_patent_attorney: m.person_credential_kr_patent_attorney,
  kr_tax_accountant: m.person_credential_kr_tax_accountant,
  kr_cpa: m.person_credential_kr_cpa,
  kr_architect: m.person_credential_kr_architect,
  kr_professional_engineer: m.person_credential_kr_professional_engineer,
  kr_pharmacist: m.person_credential_kr_pharmacist,
  kr_korean_medicine_doctor: m.person_credential_kr_korean_medicine_doctor,
};
