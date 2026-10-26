# HXX

HXX is a web application for managing Codex conversations. The repository keeps
the web application at its root and the independently operated Codex App Server
runtime in `codex/`.

## Development

Start the Rust API from the repository root:

```sh
cargo run
```

Start the Vue development server in another terminal:

```sh
cd frontend
npm install
npm run dev
```

The Vue app proxies `/api` to the Rust server. The initial backend exposes
`GET /api/health`; App Server conversation integration is the next application
feature.

## Layout

- `src/`: Rust HTTP backend.
- `frontend/`: Vue 3 and TypeScript client.
- `codex/`: Codex App Server image, local runtime configuration, and sanitized
  configuration templates.
- `docs/architecture.md`: application and isolation design.

Local API credentials, App Server capability tokens, private configuration,
runtime binaries, and downloaded image archives are ignored by Git.
