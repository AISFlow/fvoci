/**
 * The highlighter, in a worker of its own: it is given a grammar and a text
 * and answers with the text's tokens (code-tokens.ts), or `null` when it
 * cannot colour it.
 *
 * It runs apart from the page because a grammar's patterns can take far
 * longer than a page can wait on some texts: a long run of one letter took
 * highlight.js 44 seconds in TypeScript. The page gives each text a time
 * (code-highlighter.ts) and stops the worker when it runs over; a reader's
 * page never waits on it.
 *
 * Built as a file of the app's own (`?worker`), which the content security
 * policy allows as a script of the page's origin.
 */
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import diff from 'highlight.js/lib/languages/diff';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import go from 'highlight.js/lib/languages/go';
import graphql from 'highlight.js/lib/languages/graphql';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import kotlin from 'highlight.js/lib/languages/kotlin';
import lua from 'highlight.js/lib/languages/lua';
import makefile from 'highlight.js/lib/languages/makefile';
import markdown from 'highlight.js/lib/languages/markdown';
import objectivec from 'highlight.js/lib/languages/objectivec';
import php from 'highlight.js/lib/languages/php';
import powershell from 'highlight.js/lib/languages/powershell';
import python from 'highlight.js/lib/languages/python';
import ruby from 'highlight.js/lib/languages/ruby';
import rust from 'highlight.js/lib/languages/rust';
import scss from 'highlight.js/lib/languages/scss';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import swift from 'highlight.js/lib/languages/swift';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';
import { createLowlight } from 'lowlight';

import type { CodeGrammar } from './code-languages.ts';
import { HIGHLIGHT_READY, tokensOfTree } from './code-tokens.ts';

type Grammar = typeof bash;

const GRAMMARS: Record<CodeGrammar, Grammar> = {
  bash,
  c,
  cpp,
  csharp,
  css,
  diff,
  dockerfile,
  go,
  graphql,
  ini,
  java,
  javascript,
  json,
  kotlin,
  lua,
  makefile,
  markdown,
  objectivec,
  php,
  powershell,
  python,
  ruby,
  rust,
  scss,
  shell,
  sql,
  swift,
  typescript,
  xml,
  yaml,
};

const lowlight = createLowlight(GRAMMARS);

/** What the page asks: the tokens of `text` in `grammar`, answered under `id`. */
export interface HighlightRequest {
  id: number;
  grammar: string;
  text: string;
}

/** The answer: the tokens, or `null` for a grammar it does not have or a text it could not read. */
export interface HighlightAnswer {
  id: number;
  tokens: number[] | null;
}

/** The answer to one request. A function of its own, to be tested without a worker. */
export function answer({ id, grammar, text }: HighlightRequest): HighlightAnswer {
  if (!Object.hasOwn(GRAMMARS, grammar)) return { id, tokens: null };
  try {
    return { id, tokens: tokensOfTree(lowlight.highlight(grammar, text)) };
  } catch {
    return { id, tokens: null };
  }
}

// In a worker, and only there, the global scope has `importScripts`.
if (typeof self !== 'undefined' && 'importScripts' in self) {
  self.addEventListener('message', (event: MessageEvent<HighlightRequest>) => {
    self.postMessage(answer(event.data));
  });
  self.postMessage(HIGHLIGHT_READY);
}
