import type { SearchHit as SearchTargetHit } from "@/features/workspace/search-target";

export type SearchHit = SearchTargetHit & {
  title: string;
  snippet?: Array<{ text: string; match: boolean }> | null;
  extractStatus?: string | null;
};
