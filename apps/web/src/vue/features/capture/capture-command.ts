import { z } from "zod";
import type { components } from "@/generated/api";
export type PersonalInputBody = components["schemas"]["PersonalInputBody"];
export type PersonalInputResult = components["schemas"]["PersonalInputOutput"];
export type InputIntent = PersonalInputBody["intent"];
const commandSchema = z.object({
  actorId: z.string().uuid(),
  workspaceId: z.string().uuid(),
  body: z.object({
    requestId: z.string().uuid(),
    intent: z.enum(["quick", "note", "task"]),
    title: z.string(),
    projectId: z.string().uuid().nullable().optional(),
    source: z
      .object({ documentId: z.string().uuid(), anchor: z.string().nullable().optional() })
      .nullable()
      .optional(),
  }),
});
export type PendingInputCommand = { actorId: string; workspaceId: string; body: PersonalInputBody };
export interface CommandStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}
function key(actorId: string, namespace = "personal-input"): string {
  return `fvoci:${namespace}:${actorId}`;
}
/** Store BEFORE dispatch; a transport failure or lost success never rotates the key. */
export function rememberCommand(
  storage: CommandStorage,
  command: PendingInputCommand,
  namespace?: string,
): void {
  storage.setItem(key(command.actorId, namespace), JSON.stringify(command));
}
export function recoverCommand(
  storage: CommandStorage,
  actorId: string,
  namespace?: string,
): PendingInputCommand | null {
  const raw = storage.getItem(key(actorId, namespace));
  if (!raw) return null;
  try {
    const command = commandSchema.parse(JSON.parse(raw));
    return command.actorId === actorId ? command : null;
  } catch {
    return null;
  }
}
/** Explicit abandonment or same-actor acknowledged outcome, never a late scope callback. */
export function forgetCommand(
  storage: CommandStorage,
  command: PendingInputCommand,
  namespace?: string,
): void {
  if (
    recoverCommand(storage, command.actorId, namespace)?.body.requestId === command.body.requestId
  )
    storage.removeItem(key(command.actorId, namespace));
}
/** Target A-B-A is a new lifetime; actor ABA is always retirement. */
export function inputScope() {
  let actor = "";
  let target = "";
  let actorEpoch = 0;
  let credential = "";
  let targetEpoch = 0;
  let active = true;
  return {
    bind(nextActor: string, nextTarget: string, nextCredential = "") {
      if (nextActor !== actor || nextCredential !== credential) {
        actorEpoch++;
        actor = nextActor;
        credential = nextCredential;
      }
      if (nextTarget !== target) {
        targetEpoch++;
        target = nextTarget;
      }
    },
    capture() {
      return { actor, target, actorEpoch, targetEpoch };
    },
    sameActor(scope: { actor: string; actorEpoch: number }) {
      return active && scope.actor === actor && scope.actorEpoch === actorEpoch;
    },
    current(scope: { actor: string; actorEpoch: number; target: string; targetEpoch: number }) {
      return this.sameActor(scope) && scope.target === target && scope.targetEpoch === targetEpoch;
    },
    retire() {
      active = false;
      actorEpoch++;
      targetEpoch++;
    },
  };
}
