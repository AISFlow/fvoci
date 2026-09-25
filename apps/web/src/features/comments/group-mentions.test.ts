import assert from "node:assert/strict";
import test from "node:test";
import { mentionTargetsFromBody } from "./group-mentions.ts";

const members = [
  { userId: "u-1", name: "김연구" },
  { userId: "u-2", name: "이실험" },
  { userId: "u-3", name: "박분석" },
];

const namedGroups = [
  { id: "g-lab", name: "랩팀" },
  { id: "g-short", name: "랩" },
];

test("mentionTargetsFromBody maps group @ to id and user @ to userId", () => {
  assert.deepEqual(mentionTargetsFromBody("@랩팀 그리고 @박분석", members, namedGroups), {
    mentionedUserIds: ["u-3"],
    mentionedGroupIds: ["g-lab"],
  });
});

test("mentionTargetsFromBody matches a short group name only as a whole token", () => {
  assert.deepEqual(mentionTargetsFromBody("hi @랩", members, namedGroups), {
    mentionedUserIds: [],
    mentionedGroupIds: ["g-short"],
  });
  assert.deepEqual(mentionTargetsFromBody("hi @랩팀", members, namedGroups), {
    mentionedUserIds: [],
    mentionedGroupIds: ["g-lab"],
  });
});
