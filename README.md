# Vocal Cord client code

Open-source ([AGPL-3.0](LICENSE)) home for everything Vocal Cord that
runs on a user's machine: the OpenClaw plugin, the upcoming Tauri
desktop app, future browser extensions / CLI / mobile companions, and
the Rust crates they share.

The Vocal Cord server (REST + MCP lambdas, fanout, storage, CDK,
landing page, dashboard) lives in a separate, closed-source
repository.

## Layout

```
crates/                Shared Rust crates (consumed by every client
                       that needs the same logic).
  push-core/           HMAC verify, axum HTTP receiver, /hooks/wake
                       client, in-memory rate-limit gate, dedup LRU.

clients/               One sub-directory per client.
  openclaw-plugin/     npm package: OpenClaw Gateway plugin that
                       wires up Vocal Cord's MCP server, sets up
                       webhook delivery (or falls back to WebSocket),
                       and registers a 6-hourly pull-sync cron.
                       Wraps push-core via a napi-rs binding under
                       `napi/`.
  desktop/             (placeholder) Tauri desktop app — will depend
                       on push-core directly via Cargo, no FFI hop.
```

## Build

```bash
cargo check --workspace
cargo test --workspace
npm install              # populates clients/* node_modules
```

Per-client builds: see each client's own `README.md`.

## Why is this its own repo?

Anything that runs on a user's machine should be auditable and
modifiable by that user. Splitting client code from the server keeps
the licensing story clear (AGPL-3.0 here, proprietary on the server)
and lets multiple clients reuse the same hardened Rust core without
each one re-implementing security-critical bits like HMAC
verification.

## Contributing

PRs welcome. By contributing you agree that your contributions will
be licensed under AGPL-3.0.
