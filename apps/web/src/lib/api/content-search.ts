/**
 * The server's settings for clients (`getClientConfig`), read once, and the
 * search of the bodies of cards and pages (`searchWorkspaceContent`), which
 * is there where those settings say so.
 */
import type { ContentSearch, SearchAsk, SearchHit } from '../state/content-search.ts';

import type { Schemas } from './convert.ts';
import type { Http } from './http.ts';
import { underWorkspace } from './requests.ts';

export class ClientConfig {
  readonly #http: Http;
  #config: Promise<Schemas['ClientConfig']> | undefined;

  constructor(http: Http) {
    this.#http = http;
  }

  /** The settings, read once; a failed reading is tried again next time. */
  read(): Promise<Schemas['ClientConfig']> {
    this.#config ??= this.#http.request<Schemas['ClientConfig']>('/api/v1/client-config');
    this.#config.catch(() => {
      this.#config = undefined;
    });
    return this.#config;
  }
}

export class ServerContentSearch implements ContentSearch {
  readonly #http: Http;
  readonly #config: ClientConfig;

  constructor(http: Http, config: ClientConfig) {
    this.#http = http;
    this.#config = config;
  }

  async enabled(): Promise<boolean> {
    return (await this.#config.read()).searchEnabled;
  }

  async find(workspaceId: string, ask: SearchAsk): Promise<SearchHit[]> {
    const hits = await this.#http.request<Schemas['SearchHit'][]>(
      underWorkspace('/search'),
      { workspaceId },
      { query: { q: ask.query, kind: ask.kind, limit: ask.limit } },
    );
    return hits.map((hit) => ({
      id: hit.id,
      kind: hit.kind,
      containerId: hit.containerId,
      containerKind: hit.containerKind,
      containerTitle: hit.containerTitle,
      title: hit.title,
      ...(hit.number === undefined || hit.number === null ? {} : { number: hit.number }),
      snippet: hit.snippet.map((part) => ({ text: part.text, matched: part.matched })),
    }));
  }
}
