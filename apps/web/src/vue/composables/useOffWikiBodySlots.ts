import { t } from "@fvoci/i18n";
import { ref } from "vue";
import {
  ownerKey,
  retainedScope,
  type RetainedOffDraft,
} from "@/features/documents/off-wiki-draft-slot";
import { shareDownloadName, triggerDownload } from "@/lib/share";
import { copyText } from "../features/settings/clipboard";

// This tab's OFF draft slots and the local export of a retained draft. Nothing
// here imports the editor: a screen without one checks for slots here and
// loads the editor only to read one (useOffWikiBody).

// Slots the tab storage refused (quota, blocked), kept in this page's memory
// under the same key: a denial, a retry, regained access or a remount in this
// page still restores them. Leaving, closing or reloading the page loses them.
const unstoredSlots = new Map<string, string>();
type TabSlots = Pick<Storage, "length" | "key" | "getItem" | "setItem" | "removeItem">;
/** This tab's draft slots: sessionStorage, then this page's memory for writes it refused. */
export function tabStorage(): TabSlots {
  let storage: Storage | null;
  try {
    storage = window.sessionStorage;
  } catch {
    storage = null;
  }
  const keys = () => {
    const all = new Set(unstoredSlots.keys());
    for (let index = 0; storage && index < storage.length; index++) {
      const key = storage.key(index);
      if (key !== null) all.add(key);
    }
    return [...all];
  };
  return {
    get length() {
      return keys().length;
    },
    key: (index) => keys()[index] ?? null,
    // Memory is written only by a refused write, so it is never the older copy.
    getItem: (key) => unstoredSlots.get(key) ?? storage?.getItem(key) ?? null,
    setItem(key, value) {
      try {
        if (!storage) throw new Error("Tab storage unavailable");
        storage.setItem(key, value);
        unstoredSlots.delete(key);
      } catch (error) {
        // The draft still reports the failure, so its warning stays.
        unstoredSlots.set(key, value);
        throw error;
      }
    },
    removeItem(key) {
      unstoredSlots.delete(key);
      storage?.removeItem(key);
    },
  };
}
/** Whether only this page's memory holds the slot under this key. */
export function unstored(key: string): boolean {
  return unstoredSlots.has(key);
}
/** The draft as this page holds it, marked when only its memory does. */
export function held(draft: RetainedOffDraft | null): RetainedOffDraft | null {
  return draft && unstored(ownerKey(draft.owner)) ? { ...draft, unstored: true } : draft;
}
/** Whether this tab holds a slot of one actor and credential, by key alone:
 * no slot is read. */
export function holdsRetainedDrafts(actorId: string, credentialId: string): boolean {
  const scope = retainedScope(actorId, credentialId);
  try {
    const storage = tabStorage();
    for (let index = 0; index < storage.length; index++)
      if (storage.key(index)?.startsWith(scope)) return true;
  } catch {
    /* Unavailable storage holds nothing. */
  }
  return false;
}

/** Local copy/download of a retained draft; neither path reaches the server. */
export function useRetainedDraftExport() {
  const note = ref<string | null>(null);
  async function run(draft: RetainedOffDraft | null, how: "copy" | "download", title?: string) {
    note.value = null;
    if (!draft) return;
    try {
      if (how === "copy") {
        await copyText(draft.markdown);
        note.value = t("doc.off.copyDraftDone");
      } else
        triggerDownload(
          shareDownloadName(title?.trim() || draft.excerpt || t("doc.off.mine"), "md"),
          new Blob([draft.markdown], { type: "text/markdown;charset=utf-8" }),
        );
    } catch {
      note.value = t("doc.off.copyDraftFailed");
    }
  }
  return { note, run };
}
