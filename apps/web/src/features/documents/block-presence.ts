// apps/web/src/features/documents/block-presence.ts

/* WHY: #571 — 로컬 캐럿이 선 블록 id 를 awareness 에 알린다. 읽는 쪽은 collab-session 의 blockIdOf.
 * apps/web 은 @tiptap/* 를 의존하지 않으므로(라우트 청크 분리) ProseMirror 타입을 끌어오지 않고
 * blockIdAt 이 실제로 읽는 만큼만 구조적으로 선언한다 — Tiptap Editor 가 그대로 만족한다. */

interface BlockPresencePos {
	depth: number;
	node(depth: number): { attrs: Record<string, unknown> };
}

export interface BlockPresenceState {
	selection: { $from: BlockPresencePos };
}

export interface BlockPresenceEditor {
	state: BlockPresenceState;
	on(event: "selectionUpdate", cb: () => void): void;
	off(event: "selectionUpdate", cb: () => void): void;
}

interface BlockPresenceAwareness {
	setLocalStateField(field: string, value: unknown): void;
}

export function isBlockPresenceAwareness(
	value: unknown,
): value is BlockPresenceAwareness {
	if (typeof value !== "object" || value === null) return false;
	return typeof Reflect.get(value, "setLocalStateField") === "function";
}

/** WHY: PresenceBadges 는 `[data-id]` 로 블록을 찾는다 — 그 값과 같으려면 가장 안쪽 UniqueID 노드다. */
export function blockIdAt(state: BlockPresenceState): string | null {
	const pos = state.selection.$from;
	for (let depth = pos.depth; depth > 0; depth -= 1) {
		const id = pos.node(depth).attrs.id;
		if (typeof id === "string" && id.length > 0) return id;
	}
	return null;
}

/** WHY: 블록을 옮길 때만 쓴다 — 같은 블록 안 커서 이동까지 방송하면 awareness 트래픽이 타건 단위가 된다. */
export function bindBlockPresence(
	editor: BlockPresenceEditor,
	awareness: BlockPresenceAwareness,
): () => void {
	let last: string | null = null;
	const sync = () => {
		const id = blockIdAt(editor.state);
		if (id === last) return;
		last = id;
		awareness.setLocalStateField("block", id === null ? null : { id });
	};
	editor.on("selectionUpdate", sync);
	sync();
	return () => {
		editor.off("selectionUpdate", sync);
		awareness.setLocalStateField("block", null);
	};
}
