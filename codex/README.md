# Codex Runtime

This directory contains the Codex App Server container definition and local
runtime configuration. The checked-in files are templates with no organization
specific proxy, API URL, credentials, or network aliases.

## Local setup

1. Copy `docker-compose.yaml.example` to `docker-compose.yaml` and `.env.example`
   to `.env`.
2. Add the API key and any optional proxy or Docker network settings to `.env`.
3. Put matching Linux binaries named `codex` and `codex-code-mode-host` in
   `codex-bin/`.
4. Copy `config.toml.example` to `home/config.toml` and set the provider URL and
   model ID supported by that gateway.
5. Create `secrets/app-server-token` with a high-entropy capability token and
   make it readable by the container's UID 10001.
6. Build the image with `docker build -t codex-web:latest .`, then start it with
   `docker compose up -d` or `docker-compose up -d`.

The App Server listens on port 4500 inside the configured Docker network and
requires the capability token in the WebSocket `Authorization: Bearer` header.
Do not publish the port directly to the public network.

Codex CLI and `codex-code-mode-host` must come from the same release and match
the container's Linux architecture. Keep downloaded binaries and local runtime
state out of Git; `.gitignore` already excludes those paths.
