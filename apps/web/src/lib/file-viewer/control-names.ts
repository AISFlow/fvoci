/**
 * The names the app gives the kinds of control (Hangul's 조판 부호) that the
 * editor's frame counts by the id the format gives each (`tbl`, `pgnp`): the
 * frame sends the ids only, as rhwp lists them, without the trailing spaces
 * the format pads them with. An id the app does not know is shown as it is.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

const NAMES: Record<string, () => string> = {
  secd: m.file_editor_control_secd,
  cold: m.file_editor_control_cold,
  tbl: m.file_editor_control_tbl,
  gso: m.file_editor_control_gso,
  eqed: m.file_editor_control_eqed,
  head: m.file_editor_control_head,
  foot: m.file_editor_control_foot,
  fn: m.file_editor_control_fn,
  en: m.file_editor_control_en,
  atno: m.file_editor_control_atno,
  nwno: m.file_editor_control_nwno,
  pghd: m.file_editor_control_pghd,
  pgct: m.file_editor_control_pgct,
  pgnp: m.file_editor_control_pgnp,
  idxm: m.file_editor_control_idxm,
  bokm: m.file_editor_control_bokm,
  tcps: m.file_editor_control_tcps,
  tdut: m.file_editor_control_tdut,
  tcmt: m.file_editor_control_tcmt,
  form: m.file_editor_control_form,
  '%hlk': m.file_editor_control_hyperlink,
  '%clk': m.file_editor_control_clickhere,
};

/** What the writer is shown for the kind of control `id` names. */
export function controlName(id: string): string {
  const named = Object.hasOwn(NAMES, id) ? NAMES[id] : undefined;
  if (named) return named();
  if (id === '') return m.file_editor_control_unknown();
  return id.startsWith('%')
    ? m.file_editor_control_field({ id })
    : m.file_editor_control_other({ id });
}
