const MENTION_CAP = 50;
const NAME_CHAR = /[\p{L}\p{N}_]/u;

function isMentioned(body: string, name: string): boolean {
  const token = `@${name}`;
  let from = 0;
  while (from <= body.length) {
    const at = body.indexOf(token, from);
    if (at === -1) return false;
    const end = at + token.length;
    const next = body[end];
    if (next === undefined || !NAME_CHAR.test(next)) return true;
    from = at + 1;
  }
  return false;
}

export function mentionTargetsFromBody(
  body: string,
  members: ReadonlyArray<{ userId: string; name: string }>,
  groups: ReadonlyArray<{ id: string; name: string }>,
): { mentionedUserIds: string[]; mentionedGroupIds: string[] } {
  const mentionedUserIds: string[] = [];
  for (const member of members) {
    if (member.name.length === 0) continue;
    if (isMentioned(body, member.name)) mentionedUserIds.push(member.userId);
  }
  const mentionedGroupIds: string[] = [];
  for (const group of groups) {
    if (group.name.length === 0) continue;
    if (isMentioned(body, group.name)) mentionedGroupIds.push(group.id);
  }
  return {
    mentionedUserIds: mentionedUserIds.slice(0, MENTION_CAP),
    mentionedGroupIds: mentionedGroupIds.slice(0, MENTION_CAP),
  };
}
