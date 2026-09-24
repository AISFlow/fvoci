// packages/editor/src/react/attachment-view.tsx
import { uuid } from "../uuid.js";
import { t } from "@fvoci/i18n";
import {
	type ChangeEvent,
	createContext,
	type ReactNode,
	useCallback,
	useContext,
	useEffect,
	useEffectEvent,
	useRef,
	useState,
} from "react";
import { formatBytes } from "./format-bytes.js";
import {
	type PreviewAttachment,
	pickPreviewRenderer,
} from "./preview-registry.js";

export function isStoredAttachmentId(id: string): boolean {
	return uuid.safeParse(id).success;
}

export function decodeFilename(name: string): string {
	if (!/%[0-9A-Fa-f]{2}/.test(name)) return name;
	try {
		return decodeURIComponent(name);
	} catch {
		return name;
	}
}

export interface AttachmentUploadResult {
	id: string;
	name: string;
	image: boolean;
}

export interface AttachmentMeta {
	sizeBytes: number | null;
	mime: string;
	preview: { width: number; height: number } | null;
}

export interface AttachmentBlockBridge {
	upload(
		file: File,
		onProgress: (fraction: number) => void,
		signal?: AbortSignal,
	): Promise<AttachmentUploadResult>;
	downloadUrl(attachmentId: string): string;
	/** WHY: 치수 예약(C3)·크기/MIME 배지는 GET attachments/:id 메타에서 — 호스트가 없으면 카드는 이름만. */
	attachmentMeta?(attachmentId: string): Promise<AttachmentMeta | null>;
}

const META_RETRY_MS = [1000, 2000, 4000];

/** WHY: 썸네일 잡은 완료 뒤에 돈다 — preview 가 생길 때까지만 1·2·4초 재시도(R5·G2-1). */
export function useAttachmentMeta(
	load: (() => Promise<AttachmentMeta | null>) | undefined,
	waitForPreview: boolean,
): AttachmentMeta | null {
	const [meta, setMeta] = useState<AttachmentMeta | null>(null);
	useEffect(() => {
		if (load === undefined) return;
		let cancelled = false;
		let timer: ReturnType<typeof setTimeout> | undefined;
		const attempt = (round: number) => {
			void load().then(
				(next) => {
					if (cancelled) return;
					setMeta(next);
					const delay = META_RETRY_MS[round];
					if (waitForPreview && next?.preview === null && delay !== undefined) {
						timer = setTimeout(() => attempt(round + 1), delay);
					}
				},
				() => undefined,
			);
		};
		attempt(0);
		return () => {
			cancelled = true;
			clearTimeout(timer);
		};
	}, [load, waitForPreview]);
	return meta;
}

/** WHY: load 는 effect 의존성이라 렌더마다 새로 만들면 메타 GET 이 무한 반복된다(실측 10초 1,375회) — 브리지·id 로만 바뀐다. */
function useMetaLoader(
	bridge: AttachmentBlockBridge | null,
	id: string,
): (() => Promise<AttachmentMeta | null>) | undefined {
	const attachmentMeta = bridge?.attachmentMeta;
	return useCallback(
		() => attachmentMeta?.(id) ?? Promise.resolve(null),
		[attachmentMeta, id],
	);
}

export const AttachmentBlockContext =
	createContext<AttachmentBlockBridge | null>(null);

export type AttachmentCardState =
	| { kind: "placeholder" }
	| { kind: "picker" }
	| { kind: "uploading"; name: string; fraction: number }
	| { kind: "error"; name: string; message: string }
	| {
			kind: "stored";
			name: string;
			image: boolean;
			href: string | null;
			sizeBytes?: number | null;
			mime?: string;
	  };

export function AttachmentCardView(props: {
	state: AttachmentCardState;
	onPick?: () => void;
	onRetry?: () => void;
	onCancel?: () => void;
	onRemove?: () => void;
}): ReactNode {
	const { state } = props;
	switch (state.kind) {
		case "placeholder":
			return (
				<div className="afn-attachment" data-state="placeholder">
					<span className="afn-attachment-icon" aria-hidden>
						📎
					</span>
					<span className="afn-attachment-name">
						{t("editor.attach.unselected")}
					</span>
					{props.onPick && (
						<button
							type="button"
							className="afn-attachment-button min-h-11"
							onMouseDown={(e) => e.preventDefault()}
							onClick={props.onPick}
						>
							{t("editor.attach.pick")}
						</button>
					)}
					{props.onRemove && (
						<button
							type="button"
							className="afn-attachment-button min-h-11"
							onMouseDown={(e) => e.preventDefault()}
							onClick={props.onRemove}
						>
							{t("editor.attach.remove")}
						</button>
					)}
				</div>
			);
		case "picker":
			return (
				<div className="afn-attachment" data-state="picker">
					<span className="afn-attachment-icon" aria-hidden>
						📎
					</span>
					<button
						type="button"
						className="afn-attachment-button min-h-11"
						onMouseDown={(e) => e.preventDefault()}
						onClick={props.onPick}
					>
						{t("editor.attach.pick")}
					</button>
				</div>
			);
		case "uploading": {
			const percent = Math.round(state.fraction * 100);
			return (
				<div className="afn-attachment" data-state="uploading">
					<span className="afn-attachment-icon" aria-hidden>
						📎
					</span>
					<span className="afn-attachment-name">{state.name}</span>
					<span
						className="afn-attachment-progress"
						role="progressbar"
						aria-label={state.name}
						aria-valuenow={percent}
						aria-valuemin={0}
						aria-valuemax={100}
					>
						<span
							className="afn-attachment-progress-bar"
							style={{ width: `${percent}%` }}
						/>
					</span>
					<span className="afn-attachment-percent">{percent}%</span>
					{props.onCancel && (
						<button
							type="button"
							className="afn-attachment-button min-h-11"
							onMouseDown={(e) => e.preventDefault()}
							onClick={props.onCancel}
						>
							{t("editor.attach.cancel")}
						</button>
					)}
				</div>
			);
		}
		case "error":
			return (
				<div className="afn-attachment" data-state="error" role="alert">
					<span className="afn-attachment-icon" aria-hidden>
						⚠️
					</span>
					<span className="afn-attachment-name">{state.name}</span>
					<span className="afn-attachment-error">{state.message}</span>
					<button
						type="button"
						className="afn-attachment-button min-h-11"
						onMouseDown={(e) => e.preventDefault()}
						onClick={props.onRetry}
					>
						{t("editor.attach.retry")}
					</button>
					{props.onRemove && (
						<button
							type="button"
							className="afn-attachment-button min-h-11"
							onMouseDown={(e) => e.preventDefault()}
							onClick={props.onRemove}
						>
							{t("editor.attach.remove")}
						</button>
					)}
				</div>
			);
		case "stored": {
			const badge = [
				typeof state.sizeBytes === "number" ? formatBytes(state.sizeBytes) : "",
				state.mime ?? "",
			]
				.filter((s) => s.length > 0)
				.join(" · ");
			const body = (
				<>
					<span className="afn-attachment-icon" aria-hidden>
						{state.image ? "🖼️" : "📎"}
					</span>
					<span className="afn-attachment-name">
						{decodeFilename(state.name) || t("editor.block.attachment")}
					</span>
					{badge.length > 0 && (
						<span className="afn-attachment-badge">{badge}</span>
					)}
				</>
			);
			return state.href ? (
				<a
					className="afn-attachment"
					data-state="stored"
					href={state.href}
					download
				>
					{body}
				</a>
			) : (
				<div className="afn-attachment" data-state="stored">
					{body}
				</div>
			);
		}
	}
}

type AttachmentBlockProps = PreviewAttachment;

function isAbortError(err: unknown): boolean {
	return err instanceof Error && err.name === "AbortError";
}

function StoredCard(props: {
	blockProps: AttachmentBlockProps;
	bridge: AttachmentBlockBridge | null;
}): ReactNode {
	const { bridge, blockProps } = props;
	const meta = useAttachmentMeta(useMetaLoader(bridge, blockProps.id), false);
	return (
		<AttachmentCardView
			state={{
				kind: "stored",
				name: blockProps.name,
				image: blockProps.image,
				href: bridge ? bridge.downloadUrl(blockProps.id) : null,
				sizeBytes: meta?.sizeBytes,
				mime: meta?.mime,
			}}
		/>
	);
}

export function AttachmentBlockView(props: {
	initialFile?: File;
	blockProps: AttachmentBlockProps;
	readOnly: boolean;
	onUploaded: (result: AttachmentUploadResult) => void;
	onRemove?: () => void;
	onProps?: (
		next: Partial<Omit<PreviewAttachment, "id" | "name" | "image">>,
	) => void;
}): ReactNode {
	const bridge = useContext(AttachmentBlockContext);
	const metaLoader = useMetaLoader(bridge, props.blockProps.id);
	const inputRef = useRef<HTMLInputElement>(null);
	const abortRef = useRef<AbortController | null>(null);
	const [phase, setPhase] = useState<
		| { kind: "idle" }
		| { kind: "uploading"; name: string; fraction: number }
		| { kind: "error"; name: string; file: File }
	>({ kind: "idle" });

	/* WHY: #644 F7 — 노드 삭제·Mod-z·에디터 파괴로 뷰가 사라져도 in-flight 업로드는
	 * 끝까지 달려 사라진 노드에 되쓰기를 시도한다. 언마운트에서 끊어 고아 업로드를 막는다. */
	useEffect(() => () => abortRef.current?.abort(), []);

	const startUpload = (file: File): void => {
		if (!bridge) return;
		abortRef.current?.abort();
		const ac = new AbortController();
		abortRef.current = ac;
		setPhase({ kind: "uploading", name: file.name, fraction: 0 });
		void bridge
			.upload(
				file,
				(fraction) => {
					if (ac.signal.aborted) return;
					setPhase({ kind: "uploading", name: file.name, fraction });
				},
				ac.signal,
			)
			.then((result) => {
				if (ac.signal.aborted) return;
				setPhase({ kind: "idle" });
				props.onUploaded(result);
			})
			.catch((err: unknown) => {
				if (ac.signal.aborted || isAbortError(err)) return;
				setPhase({ kind: "error", name: file.name, file });
			});
	};

	const uploadInitialFile = useEffectEvent(startUpload);
	useEffect(() => {
		if (props.initialFile) uploadInitialFile(props.initialFile);
	}, [props.initialFile]);

	const onFileChange = (event: ChangeEvent<HTMLInputElement>): void => {
		const file = event.target.files?.[0];
		event.target.value = "";
		if (file) startUpload(file);
		else props.onRemove?.();
	};

	if (isStoredAttachmentId(props.blockProps.id)) {
		if (bridge) {
			const renderer = pickPreviewRenderer(props.blockProps);
			if (renderer) {
				return renderer.render(props.blockProps, {
					downloadUrl: bridge.downloadUrl(props.blockProps.id),
					meta: metaLoader,
					updateProps: props.readOnly ? undefined : props.onProps,
				});
			}
		}
		return <StoredCard blockProps={props.blockProps} bridge={bridge} />;
	}
	/* WHY: #644 리뷰 #4 — 진행 중 업로드는 readOnly 로 뒤집혀도 계속 달린다. 읽기 전용
	 * 분기를 먼저 두면 진행률·취소가 사라져 사용자가 끊을 방법 없이 첨부가 붙는다. */
	if (phase.kind === "uploading") {
		return (
			<AttachmentCardView
				state={{
					kind: "uploading",
					name: phase.name,
					fraction: phase.fraction,
				}}
				onCancel={() => {
					abortRef.current?.abort();
					setPhase({ kind: "idle" });
					props.onRemove?.();
				}}
			/>
		);
	}
	if (!bridge || props.readOnly) {
		return <AttachmentCardView state={{ kind: "placeholder" }} />;
	}
	if (phase.kind === "error") {
		return (
			<AttachmentCardView
				state={{
					kind: "error",
					name: phase.name,
					message: t("editor.attach.failed"),
				}}
				onRetry={() => startUpload(phase.file)}
				onRemove={props.onRemove}
			/>
		);
	}
	return (
		<>
			<input
				ref={inputRef}
				type="file"
				className="afn-attachment-input"
				onChange={onFileChange}
			/>
			<AttachmentCardView
				state={{ kind: "placeholder" }}
				onPick={() => inputRef.current?.click()}
				onRemove={props.onRemove}
			/>
		</>
	);
}
