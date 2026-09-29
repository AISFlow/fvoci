/* Entity kinds that mention and embed nodes reference, and the resolver the
 * host supplies to label them. Framework-free: the editor extension factory
 * and the React blocks both read these. */

export const EMBED_ENTITIES = ["document", "task", "project", "url"] as const;
export type EmbedEntity = (typeof EMBED_ENTITIES)[number];

export function isEmbedEntity(value: string): value is EmbedEntity {
	for (const entity of EMBED_ENTITIES) {
		if (entity === value) return true;
	}
	return false;
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
