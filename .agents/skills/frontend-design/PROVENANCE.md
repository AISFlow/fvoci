# Frontend design skill provenance

Upstream snapshot from [anthropics/skills](https://github.com/anthropics/skills/tree/41bbe19d1a1a7eaab5e7bb9050a417e5c6cffc8f/skills/frontend-design), commit `41bbe19d1a1a7eaab5e7bb9050a417e5c6cffc8f`. The table is that snapshot, not the current `SKILL.md` size:

| Local file  | Upstream path                      | Git blob SHA-1                           | SHA-256                                                          | Bytes |
| ----------- | ---------------------------------- | ---------------------------------------- | ---------------------------------------------------------------- | ----- |
| SKILL.md    | skills/frontend-design/SKILL.md    | a5333457c414d20d625f307df945842c0952ecc3 | d91970639e9f5c37682ac7ab60094d35f1c7c1f38d731bd56396563aee10c1d3 | 9390  |
| LICENSE.txt | skills/frontend-design/LICENSE.txt | f433b1a53f5b830a205fd2df78e2b34974656c7b | 0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594 | 10174 |

Both raw downloads were checked against the blob IDs in GitHub's complete recursive tree at the pinned commit, then compared byte-for-byte with the files as first installed. The upstream `LICENSE.txt` supplies Apache License 2.0 terms and is retained in full, still byte-identical to blob `f433b1a53f5b830a205fd2df78e2b34974656c7b`. The complete upstream tree (`truncated: false`) has no NOTICE file, including at repository root or ancestors of this skill; no upstream NOTICE attribution is omitted.

`SKILL.md` is no longer byte-identical to blob `a5333457c414d20d625f307df945842c0952ecc3`. The house-rules edit kept the upstream design text, changed the description, and placed the body under 완료 조건 / 기본 절차 / 손대지 말 것. Apache-2.0 section 4 allows that modification; this file is the notice of the change. `FVOCI-BRIEF.md` remains a separate FVOCI instruction.

Installed with the existing Codex skill installer into this project's `.agents/skills` only. No global configuration, account, model, paid service, Claude executable, dependency, or runtime setup is required. The session that installed the skill did not see it in the supplied catalog, so installation is not evidence of automatic discovery or of design acceptance.
