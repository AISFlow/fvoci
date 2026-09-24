import { type I18nKey, t } from "@fvoci/i18n";
import {
	type ChangeEvent,
	createContext,
	type FocusEvent,
	type ReactNode,
	useContext,
	useEffect,
	useRef,
	useState,
} from "react";
import { type SafeHtml, SafeHtmlView } from "./safe-html.js";

/** WHY: autoFocus 와 같은 마운트 시 focus() — 모듈 상수라 마운트에 한 번만 불린다(noAutofocus 는 dialog/popover 만 예외). */
const focusOnMount = (el: HTMLElement | null) => el?.focus();

/* WHY: #644 리뷰 #2 — 권한이 사라지면 편집 상태도 닫는다. 남겨 두면 권한이 돌아올 때
 * 사용자가 열지도 않은 편집기가 낡은 값으로 되살아나 캐럿을 훔친다.
 * WHY: #658 — React 는 언마운트 때 blur 를 쏘지 않으므로, 닫기 전에 마지막 초안을 커밋한다. */
function useBlockEdit<T>(editable: boolean, commit: (draft: T) => void) {
	const [editing, setEditing] = useState(false);
	const draft = useRef<T | null>(null);
	useEffect(() => {
		if (editable) return;
		setEditing(false);
		const pending = draft.current;
		draft.current = null;
		if (pending !== null) commit(pending);
	}, [editable, commit]);
	return { editing, setEditing, draft };
}

export function MathBlockView({
	latex,
	editable,
	html,
	failed,
	onCommit,
}: {
	latex: string;
	editable: boolean;
	html: SafeHtml | null;
	failed: boolean;
	onCommit: (latex: string) => void;
}) {
	const { editing, setEditing, draft } = useBlockEdit(editable, onCommit);
	if (editing && editable) {
		return (
			<textarea
				className="afn-math-edit"
				ref={focusOnMount}
				defaultValue={latex}
				aria-label={t("editor.math.latex")}
				onChange={(e) => {
					draft.current = e.target.value === latex ? null : e.target.value;
				}}
				onBlur={(e) => {
					draft.current = null;
					onCommit(e.target.value);
					setEditing(false);
				}}
			/>
		);
	}
	const empty = latex.trim() === "";
	/* WHY: MathML 은 브라우저가 그대로 읽는다 — role="img" 로 덮으면 그 접근성을 버린다. */
	const inner = html ? (
		<SafeHtmlView html={html} />
	) : (
		<pre>{empty ? t("editor.math.empty") : latex}</pre>
	);
	const shell = {
		className: "afn-math",
		"data-empty": empty ? "true" : undefined,
		"data-failed": failed ? "true" : undefined,
	};
	if (!editable) return <div {...shell}>{inner}</div>;
	return (
		<button
			type="button"
			{...shell}
			title={t("editor.math.edit")}
			onMouseDown={(e) => e.preventDefault()}
			onClick={() => setEditing(true)}
		>
			{inner}
		</button>
	);
}

/* WHY: #688 — 블록 수식의 편집기는 다줄 textarea 지만 인라인은 문장 안에 앉는다. 한 줄
 * <input> 이라 Enter 가 곧 커밋이고, 개행이 들어올 자리가 없다. 표시부는 블록과 같이 <button>
 * 이라 클릭·Enter·Space 가 모두 편집을 연다(네이티브 버튼 활성화). */
export function MathInlineView({
	latex,
	editable,
	html,
	failed,
	onCommit,
}: {
	latex: string;
	editable: boolean;
	html: SafeHtml | null;
	failed: boolean;
	onCommit: (latex: string) => void;
}) {
	const [editing, setEditing] = useState(false);
	// WHY: #644 리뷰 #2 — 권한이 사라지면 편집 상태도 닫는다(블록 수식과 같은 이유).
	useEffect(() => {
		if (!editable) setEditing(false);
	}, [editable]);
	if (editing && editable) {
		return (
			<input
				className="afn-math-inline-edit"
				ref={focusOnMount}
				defaultValue={latex}
				aria-label={t("editor.math.latex")}
				onKeyDown={(e) => {
					if (e.key === "Enter") e.currentTarget.blur();
				}}
				onBlur={(e) => {
					onCommit(e.target.value);
					setEditing(false);
				}}
			/>
		);
	}
	const empty = latex.trim() === "";
	/* WHY: MathML 은 브라우저가 그대로 읽는다 — role="img" 로 덮으면 그 접근성을 버린다. */
	const inner = html ? <SafeHtmlView html={html} /> : empty ? "$…$" : latex;
	const shell = {
		className: "afn-math-inline",
		"data-empty": empty ? "true" : undefined,
		"data-failed": failed ? "true" : undefined,
	};
	if (!editable) return <span {...shell}>{inner}</span>;
	return (
		<button
			type="button"
			{...shell}
			title={t("editor.math.edit")}
			onMouseDown={(e) => e.preventDefault()}
			onClick={() => setEditing(true)}
		>
			{inner}
		</button>
	);
}

export function MermaidBlockView({
	code,
	editable,
	svg,
	failed,
	onCommit,
}: {
	code: string;
	editable: boolean;
	svg: SafeHtml | null;
	failed: boolean;
	onCommit: (code: string) => void;
}) {
	const { editing, setEditing, draft } = useBlockEdit(editable, onCommit);
	if (editing && editable) {
		return (
			<textarea
				className="afn-math-edit"
				ref={focusOnMount}
				defaultValue={code}
				aria-label={t("editor.mermaid.code")}
				onChange={(e) => {
					draft.current = e.target.value === code ? null : e.target.value;
				}}
				onBlur={(e) => {
					draft.current = null;
					onCommit(e.target.value);
					setEditing(false);
				}}
			/>
		);
	}
	const enter = () => {
		if (editable) setEditing(true);
	};
	const firstLine = code
		.split("\n")
		.map((line) => line.trim())
		.find((line) => line !== "");
	const diagramLabel = firstLine
		? t("editor.mermaid.diagram", { line: firstLine })
		: t("editor.mermaid.empty");
	if (svg && !failed) {
		const diagram = (
			<SafeHtmlView
				className="afn-mermaid"
				role="img"
				aria-label={diagramLabel}
				html={svg}
			/>
		);
		if (!editable) return diagram;
		return (
			<button
				type="button"
				className="afn-embed-host"
				title={t("editor.mermaid.edit")}
				onMouseDown={(e) => e.preventDefault()}
				onClick={enter}
			>
				{diagram}
			</button>
		);
	}
	const source = <pre>{code || t("editor.mermaid.sourceEmpty")}</pre>;
	if (!editable) {
		return (
			<div
				className="afn-mermaid-source"
				data-failed={failed ? "true" : undefined}
			>
				{source}
			</div>
		);
	}
	return (
		<button
			type="button"
			className="afn-mermaid-source"
			data-failed={failed ? "true" : undefined}
			title={t("editor.mermaid.edit")}
			onMouseDown={(e) => e.preventDefault()}
			onClick={enter}
		>
			{source}
		</button>
	);
}

const EMBED_ENTITIES = ["document", "task", "project", "url"] as const;
export type EmbedEntity = (typeof EMBED_ENTITIES)[number];

const EMBED_KEY = {
	document: "editor.embed.document",
	task: "editor.embed.task",
	project: "editor.embed.project",
	url: "editor.link",
} as const satisfies Record<EmbedEntity, I18nKey>;

export function isEmbedEntity(value: string): value is EmbedEntity {
	for (const entity of EMBED_ENTITIES) {
		if (entity === value) return true;
	}
	return false;
}

function isEmbedHttpUrl(value: string): boolean {
	try {
		const parsed = new URL(value);
		return parsed.protocol === "http:" || parsed.protocol === "https:";
	} catch {
		return false;
	}
}

export function resolveEmbedProps(
	raw: string,
	selected: EmbedEntity,
): { entity: EmbedEntity; ref: string } {
	const ref = raw.trim();
	if (isEmbedHttpUrl(ref)) return { entity: "url", ref };
	return { entity: selected, ref };
}

function readEmbedEdit(container: HTMLElement): {
	entity: EmbedEntity;
	ref: string;
} {
	const input = container.querySelector("textarea");
	const select = container.querySelector("select");
	const raw = input instanceof HTMLTextAreaElement ? input.value : "";
	const selected =
		select instanceof HTMLSelectElement && isEmbedEntity(select.value)
			? select.value
			: "document";
	return resolveEmbedProps(raw, selected);
}

const MENTION_ENTITIES = [
	"user",
	"document",
	"task",
	"project",
	"group",
] as const;
export type MentionEntity = (typeof MENTION_ENTITIES)[number];

export function isMentionEntity(value: string): value is MentionEntity {
	for (const entity of MENTION_ENTITIES) {
		if (entity === value) return true;
	}
	return false;
}

export type EntitySnapshot = {
	label: string;
	icon: string;
	status?: string;
};

export type EntityResolver = (
	entity: MentionEntity,
	id: string,
) => Promise<EntitySnapshot | null>;

export const EntityResolverContext = createContext<EntityResolver | null>(null);

const EMBED_ICON: Record<Exclude<EmbedEntity, "url">, string> = {
	document: "📄",
	task: "☑",
	project: "📁",
};

export type UrlEmbedRenderer = (url: string) => ReactNode;

export const UrlEmbedContext = createContext<UrlEmbedRenderer | null>(null);

export type EmbedCardState =
	| { state: "loading" }
	| { state: "inaccessible" }
	| { state: "plain"; ref: string }
	| { state: "resolved"; snapshot: EntitySnapshot };

export function EmbedCardView({
	entity,
	state,
}: {
	entity: EmbedEntity;
	state: EmbedCardState;
}) {
	const kind = t(EMBED_KEY[entity]);
	const icon =
		entity === "url"
			? ""
			: state.state === "resolved" && state.snapshot.icon
				? state.snapshot.icon
				: EMBED_ICON[entity];
	const refText =
		state.state === "plain"
			? state.ref || t("editor.embed.noRef")
			: state.state === "loading"
				? t("editor.embed.loading")
				: state.state === "inaccessible"
					? t("editor.embed.inaccessible", { kind })
					: state.snapshot.label;
	const status = state.state === "resolved" ? state.snapshot.status : undefined;
	return (
		<div
			className={
				state.state === "inaccessible"
					? "afn-embed afn-embed-inaccessible"
					: "afn-embed"
			}
			data-entity={entity}
		>
			{icon ? (
				<span className="afn-embed-icon" aria-hidden="true">
					{icon}
				</span>
			) : null}
			<span className="afn-embed-kind">{kind}</span>
			<span className="afn-embed-ref">{refText}</span>
			{status ? <span className="afn-embed-meta">{status}</span> : null}
		</div>
	);
}

function UrlEmbedBlock({ url }: { url: string }) {
	const render = useContext(UrlEmbedContext);
	if (render) return <>{render(url)}</>;
	return <EmbedCardView entity="url" state={{ state: "plain", ref: url }} />;
}

function ResolvedEmbed({
	entity,
	refValue,
}: {
	entity: Exclude<EmbedEntity, "url">;
	refValue: string;
}) {
	const resolver = useContext(EntityResolverContext);
	const [state, setState] = useState<EmbedCardState>(
		resolver && refValue
			? { state: "loading" }
			: { state: "plain", ref: refValue },
	);
	useEffect(() => {
		if (!resolver || !refValue) {
			setState({ state: "plain", ref: refValue });
			return;
		}
		let cancelled = false;
		setState((prev) =>
			prev.state === "resolved" || prev.state === "inaccessible"
				? prev
				: { state: "loading" },
		);
		void resolver(entity, refValue).then(
			(snap) => {
				if (cancelled) return;
				setState(
					snap
						? { state: "resolved", snapshot: snap }
						: { state: "inaccessible" },
				);
			},
			() => {
				if (!cancelled) setState({ state: "inaccessible" });
			},
		);
		return () => {
			cancelled = true;
		};
	}, [resolver, entity, refValue]);
	return <EmbedCardView entity={entity} state={state} />;
}

function EmbedCard({
	entity,
	refValue,
}: {
	entity: EmbedEntity;
	refValue: string;
}) {
	if (entity === "url") {
		if (refValue) return <UrlEmbedBlock url={refValue} />;
		return (
			<EmbedCardView entity="url" state={{ state: "plain", ref: refValue }} />
		);
	}
	return (
		<ResolvedEmbed
			key={`${entity}:${refValue}`}
			entity={entity}
			refValue={refValue}
		/>
	);
}

export function EmbedBlockView({
	entity,
	refValue,
	editable,
	onCommit,
}: {
	entity: EmbedEntity;
	refValue: string;
	editable: boolean;
	onCommit: (next: { entity: EmbedEntity; ref: string }) => void;
}) {
	const { editing, setEditing, draft } = useBlockEdit(editable, onCommit);
	/* WHY: #644 리뷰 #3 — 빈 ref 자동 열기는 갓 삽입한 블록을 위한 것이다. 마운트 시점의
	 * 판정을 고정해, 권한이 돌아올 때 문서 안 빈 embed 가 전부 열려 캐럿을 뺏지 않게 한다.
	 * NodeViewProps.selected 는 게이트로 못 쓴다 — 문서 첫 자리의 atom 노드는 편집 중이
	 * 아니어도 selected 라 그대로 다시 열린다(실측). */
	const autoOpen = useRef(editable && refValue.trim() === "");
	const showEditor =
		editable && (editing || (autoOpen.current && refValue.trim() === ""));
	const rememberDraft = (
		e: ChangeEvent<HTMLSelectElement | HTMLTextAreaElement>,
	) => {
		const form = e.currentTarget.parentElement;
		if (!(form instanceof HTMLElement)) return;
		const next = readEmbedEdit(form);
		/* WHY: rev-687 F2 — 쳤다 지워 원래 값으로 돌아온 초안은 커밋할 것이 없다.
		 * 그대로 쓰면 내용이 같은 setNodeMarkup 이 빈 undo 스텝과 빈 협업 업데이트를 만든다. */
		draft.current =
			next.entity === entity && next.ref === refValue ? null : next;
	};
	const commitIfLeaving = (e: FocusEvent<HTMLElement>) => {
		const form = e.currentTarget.parentElement;
		if (!(form instanceof HTMLElement)) return;
		if (e.relatedTarget instanceof Node && form.contains(e.relatedTarget)) {
			return;
		}
		draft.current = null;
		onCommit(readEmbedEdit(form));
		setEditing(false);
	};

	if (showEditor) {
		return (
			<div className="afn-embed afn-embed-edit" data-entity={entity}>
				<select
					className="afn-embed-entity"
					defaultValue={entity}
					aria-label={t("editor.embed.kind")}
					onChange={rememberDraft}
					onBlur={commitIfLeaving}
				>
					{EMBED_ENTITIES.map((item) => (
						<option key={item} value={item}>
							{t(EMBED_KEY[item])}
						</option>
					))}
				</select>
				<textarea
					className="afn-embed-ref-input"
					ref={focusOnMount}
					defaultValue={refValue}
					aria-label={t("editor.embed.ref")}
					placeholder={t("editor.embed.placeholder")}
					onChange={rememberDraft}
					onBlur={commitIfLeaving}
				/>
			</div>
		);
	}

	const card = <EmbedCard entity={entity} refValue={refValue} />;
	if (!editable) return card;
	return (
		<button
			type="button"
			className="afn-embed-host"
			aria-label={t("editor.embed.edit")}
			onMouseDown={(e) => e.preventDefault()}
			onClick={() => setEditing(true)}
		>
			{card}
		</button>
	);
}

export function MentionView({
	entity,
	id,
	label,
}: {
	entity: MentionEntity;
	id: string;
	label: string;
}) {
	const resolver = useContext(EntityResolverContext);
	const stored = (label ?? "").trim();
	const idTrim = (id ?? "").trim();
	const empty = !stored && !idTrim;
	const [fetched, setFetched] = useState<string | null>(null);
	useEffect(() => {
		if (empty || stored || !resolver || !idTrim) {
			setFetched(null);
			return;
		}
		let cancelled = false;
		void resolver(entity, idTrim).then(
			(snap) => {
				if (!cancelled) setFetched(snap?.label ?? null);
			},
			() => {
				if (!cancelled) setFetched(null);
			},
		);
		return () => {
			cancelled = true;
		};
	}, [empty, stored, resolver, entity, idTrim]);
	if (empty) return null;
	const text = stored || fetched || "";
	return (
		<span className="afn-mention" data-entity={entity}>
			{entity === "user" || entity === "group" ? "@" : ""}
			{text}
		</span>
	);
}
