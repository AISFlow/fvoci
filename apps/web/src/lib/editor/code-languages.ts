/**
 * The languages a code block is coloured in, and what a stored language
 * names among them.
 *
 * A code block stores its language as whatever name it was given: the
 * picker's own (`rust`), or the word after a Markdown fence (```` ```C++ ````).
 * The server takes any name the rule of schema.ts allows, so a name is
 * looked up here without regard to case, among the names and the usual other
 * names of each language. A name that is not here is no failure: the block
 * shows its text without colour, as it did before there were colours, and
 * keeps its name. Nothing guesses a language from the text.
 *
 * The diagram language (`mermaid`) is not here: such a block is a picture.
 *
 * This file is small and has no grammar in it; the grammars are in the
 * highlighter's worker (code-highlight-worker.ts), fetched only when a block
 * in one of these languages is shown.
 */

/** The grammars of the highlighter, by highlight.js's own names. */
export const CODE_GRAMMARS = [
  'bash',
  'c',
  'cpp',
  'csharp',
  'css',
  'diff',
  'dockerfile',
  'go',
  'graphql',
  'ini',
  'java',
  'javascript',
  'json',
  'kotlin',
  'lua',
  'makefile',
  'markdown',
  'objectivec',
  'php',
  'powershell',
  'python',
  'ruby',
  'rust',
  'scss',
  'shell',
  'sql',
  'swift',
  'typescript',
  'xml',
  'yaml',
] as const;

export type CodeGrammar = (typeof CODE_GRAMMARS)[number];

export interface CodeLanguage {
  /** What the picker stores. */
  readonly id: string;
  /** How the language is named on the screen, in every language of the app. */
  readonly label: string;
  /** The grammar it is coloured with. */
  readonly grammar: CodeGrammar;
  /** The other names a stored block may give it, in lower case. */
  readonly aliases: readonly string[];
}

const language = (
  id: string,
  label: string,
  grammar: CodeGrammar,
  aliases: readonly string[] = [],
): CodeLanguage => ({ id, label, grammar, aliases });

/**
 * The languages, in the order the picker lists them: the web, the systems
 * languages, the scripting languages, the shells, data and configuration.
 * `shell` and `sh` are scripts here, as they are where Markdown is written;
 * a terminal's transcript, with its prompts, is `console`.
 */
export const CODE_LANGUAGES: readonly CodeLanguage[] = [
  language('javascript', 'JavaScript', 'javascript', ['js', 'jsx', 'mjs', 'cjs', 'node']),
  language('typescript', 'TypeScript', 'typescript', ['ts', 'tsx', 'mts', 'cts']),
  language('html', 'HTML', 'xml', ['htm', 'xhtml', 'vue', 'svelte']),
  language('css', 'CSS', 'css'),
  language('scss', 'SCSS', 'scss'),
  language('php', 'PHP', 'php'),
  language('graphql', 'GraphQL', 'graphql', ['gql']),
  language('c', 'C', 'c', ['h']),
  language('cpp', 'C++', 'cpp', ['c++', 'cc', 'cxx', 'hpp', 'hh', 'hxx', 'h++']),
  language('csharp', 'C#', 'csharp', ['c#', 'cs']),
  language('java', 'Java', 'java'),
  language('kotlin', 'Kotlin', 'kotlin', ['kt', 'kts']),
  language('swift', 'Swift', 'swift'),
  language('objectivec', 'Objective-C', 'objectivec', [
    'objective-c',
    'objc',
    'obj-c',
    'objective-c++',
    'objc++',
    'obj-c++',
    'mm',
  ]),
  language('go', 'Go', 'go', ['golang']),
  language('rust', 'Rust', 'rust', ['rs']),
  language('python', 'Python', 'python', ['py', 'python3', 'py3']),
  language('ruby', 'Ruby', 'ruby', ['rb']),
  language('lua', 'Lua', 'lua'),
  language('bash', 'Bash', 'bash', ['sh', 'shell', 'zsh']),
  language('console', 'Console', 'shell', ['shellsession', 'shell-session']),
  language('powershell', 'PowerShell', 'powershell', ['ps1', 'pwsh', 'ps']),
  language('sql', 'SQL', 'sql', ['mysql', 'postgresql', 'postgres', 'sqlite']),
  language('json', 'JSON', 'json', ['jsonc', 'json5']),
  language('yaml', 'YAML', 'yaml', ['yml']),
  language('xml', 'XML', 'xml', ['svg', 'plist', 'xsd', 'xsl', 'rss', 'atom']),
  language('toml', 'TOML', 'ini'),
  language('ini', 'INI', 'ini', ['cfg', 'conf']),
  language('dockerfile', 'Dockerfile', 'dockerfile', ['docker']),
  language('makefile', 'Makefile', 'makefile', ['make', 'mk', 'mak']),
  language('markdown', 'Markdown', 'markdown', ['md', 'mkd']),
  language('diff', 'Diff', 'diff', ['patch']),
];

const byName = new Map<string, CodeLanguage>(
  CODE_LANGUAGES.flatMap((known) =>
    [known.id, ...known.aliases].map((name): [string, CodeLanguage] => [name, known]),
  ),
);

/** The language a stored name names, if it is one of these; any other value names none. */
export function codeLanguageOf(stored: unknown): CodeLanguage | undefined {
  return typeof stored === 'string' ? byName.get(stored.toLowerCase()) : undefined;
}
