#!/usr/bin/env bash
# Readies a machine for Claude Code's LSP tool in this repository, once: rust-analyzer
# for the Rust code, and for the web app the TypeScript server, which sees the .svelte
# files through typescript-svelte-plugin, and the Svelte server. Run again to update.
set -euo pipefail

share="${XDG_DATA_HOME:-$HOME/.local/share}/fvoci-lsp"

rustup component add rust-analyzer

mkdir -p "$share"
[ -f "$share/package.json" ] || echo '{ "private": true }' > "$share/package.json"
npm install --prefix "$share" --no-audit --no-fund --silent \
  typescript-language-server@6.1.0 typescript@6.0.3 typescript-svelte-plugin@0.3.52 \
  svelte-language-server@0.18.6

# Claude Code takes language servers from plugins. The official typescript-lsp cannot
# be told where typescript-svelte-plugin is (pnpm's layout hides it from the TypeScript
# server), and there is none for Svelte: this marketplace of one plugin, written for
# this machine's paths, names both servers.
market="$share/marketplace"
mkdir -p "$market/.claude-plugin" "$market/web-lsp/.claude-plugin"
cat > "$market/.claude-plugin/marketplace.json" <<'JSON'
{
  "name": "fvoci-lsp",
  "owner": { "name": "fvoci" },
  "plugins": [{ "name": "web-lsp", "source": "./web-lsp" }]
}
JSON
cat > "$market/web-lsp/.claude-plugin/plugin.json" <<JSON
{
  "name": "web-lsp",
  "version": "1.0.0",
  "description": "TypeScript (with typescript-svelte-plugin) and Svelte language servers",
  "lspServers": {
    "typescript": {
      "command": "$share/node_modules/.bin/typescript-language-server",
      "args": ["--stdio"],
      "initializationOptions": {
        "plugins": [{ "name": "typescript-svelte-plugin", "location": "$share" }]
      },
      "extensionToLanguage": { ".ts": "typescript", ".js": "javascript", ".mts": "typescript" }
    },
    "svelte": {
      "command": "$share/node_modules/.bin/svelteserver",
      "args": ["--stdio"],
      "extensionToLanguage": { ".svelte": "svelte" }
    }
  }
}
JSON

claude plugin marketplace list 2>/dev/null | grep -q fvoci-lsp \
  || claude plugin marketplace add "$market"
claude plugin marketplace update fvoci-lsp
installed=$(claude plugin list 2>/dev/null)
for plugin in rust-analyzer-lsp@claude-plugins-official web-lsp@fvoci-lsp; do
  grep -qF "$plugin" <<< "$installed" || claude plugin install "$plugin"
done
