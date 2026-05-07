# `@vocalcord/openclaw-plugin`

OpenClaw plugin that wires up [Vocal Cord](https://vocalcord.io) for
real-time inbound message delivery and a periodic pull-sync. Wraps
the shared [`push-core`](../../crates/push-core) Rust crate via
[napi-rs](https://napi.rs) — the same code paths back the upcoming
Vocal Cord Tauri desktop app.

## What it does

1. **Registers the Vocal Cord MCP server** in your OpenClaw config
   (`mcp.servers.vocalcord`) so the agent can call
   `SendMessage` / `GetMessages` / `GetMessage`.
2. **Sets up real-time inbound** — by default tries the webhook
   path (low-latency, paid tier on Vocal Cord) and falls back to a
   long-lived WebSocket if the webhook can't be reached.
3. **Schedules a 6-hourly pull-sync** so pull-style channels
   (Discord, Matrix, Signal, Urbit) stay caught up even if no
   message arrives via webhook / WS.
4. **Honors 429 + Retry-After** via tool-call hooks: when Vocal
   Cord rate-limits a `GetMessages` call, subsequent calls
   short-circuit until the window passes, with no in-process
   retries piling up on top of the cron.

## Install

```bash
npm install @vocalcord/openclaw-plugin
```

In your OpenClaw gateway config:

```yaml
plugins:
  vocalcord-openclaw-plugin:
    apiKey: 00000000-0000-0000-0000-000000000000   # your Vocal Cord UUID
    hookToken: <your hooks.token>                   # OpenClaw hooks bearer token
    publicUrl: https://my-host.tailnet.ts.net      # optional; if reachable, used for webhook
```

If `publicUrl` is omitted the plugin asks Vocal Cord to use the
source IP of the configure request. When Vocal Cord can't ping back
(NAT / firewall), the plugin falls back to WebSocket automatically.

## Build from source

```bash
# from the repo root
cargo check --workspace
npm install
cd clients/openclaw-plugin
npm run build
```

This produces `dist/` (TypeScript output) and a platform-specific
`.node` (the napi-rs binding). The published npm package ships
prebuilt binaries for darwin-x64/arm64, linux-x64/arm64-gnu, and
win32-x64-msvc via `optionalDependencies`.

## License

AGPL-3.0-only. See [`../../LICENSE`](../../LICENSE).
