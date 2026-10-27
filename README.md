# HXX

HXX is a local Rust and Vue workbench for Codex App Server. The browser talks
only to the local HXX API; the Rust backend connects to the remote App Server
over its authenticated WebSocket endpoint.

## Local Development

1. Copy `hxx.toml.example` to `hxx.toml`, then set the Codex URL, token file,
   and SSH workspace settings in its `[codex]` section. Keep the App Server
   token in a separate local file.
2. Start the Rust API from the repository root:

   ```sh
   cargo run
   ```

3. Start the Vue development server in another terminal:

   ```sh
   cd frontend
   npm install
   npm run dev
   ```

Open the local URL printed by Vite. It proxies `/api` to the Rust server on
`127.0.0.1:3000`. The default database is `data/hxx.sqlite`.

## Features

- Each browser creates a stable ID in local storage; no password login is used.
- Sessions are isolated by browser ID and each browser retains at most ten.
- New sessions use a structured Markdown brief before starting a Codex thread.
- Assistant text and tool activity are refreshed during a turn with elapsed seconds.
- The dashboard summarizes current-day, current-week, and current-month requests and tokens.

## Layout

- `src/`: Rust API, SQLite persistence, and Codex App Server client.
- `frontend/`: Vue 3, TypeScript, and Element Plus interface.
- `codex/`: Codex App Server image, local runtime configuration, and templates.
- `docs/architecture.md`: application and isolation design.

Local API credentials, App Server capability tokens, private configuration,
runtime binaries, and downloaded image archives are ignored by Git.
