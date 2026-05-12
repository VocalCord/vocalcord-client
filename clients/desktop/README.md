# Vocal Cord desktop

System-tray app that connects a user's machine to Vocal Cord and runs a
local coding agent in response to inbound messages. Built on
[Tauri 2](https://tauri.app), with a React front-end.

## Status

**Scaffolded.** Connects to vocalcord via the shared
[`vocalcord-ws-client`](../../crates/vocalcord-ws-client) (WebSocket)
and [`push-core`](../../crates/push-core) (webhook receiver) crates;
sends replies via [`vocalcord-mcp-client`](../../crates/vocalcord-mcp-client).
The coding-agent integration goes through
[`lr-coding-agents`](https://github.com/LocalRouter/LocalRouter/tree/main/crates/lr-coding-agents)
— the manager wrapper in `src-tauri/src/agent_runner.rs` currently
returns `not yet wired` from its `start_session`/`say` methods; the
remaining work is constructing the real `CodingAgentManager` with our
custom `PopupTrigger` once Tauri's `AppHandle` is in scope.

## How it works

Inbound flow:

```
              ┌─── push-core::Receiver (webhook, optional) ──┐
vocalcord ─→  │                                              │  ─→  approval_router  ─→  agent_runner
              └─── vocalcord_ws_client::WsClient ────────────┘                                │
                                                                                              ↓
                                                                                          coding agent
                                                                                              │
                                                                                              ↓
                                            outbound::send_reply  ←──  agent output buffer
                                                       │
                                                       ↓
                                            POST /mcp tools/call (SendMessage)
```

System tray reflects 5 states (idle / working / attention / disconnected / error)
based on connection + agent status combined.

## Develop

```bash
cd clients/desktop
npm install
npx tauri dev          # boots the app in dev mode (Vite + cargo)
```

## Configure

Open Settings from the tray. Required:

- **API key** — your Vocal Cord account UUID.
- **Working directory** — where the agent runs (defaults to `~/Documents/vocalcord-agent-work`).
- **Agent type** — Claude Code, Codex, Gemini CLI, etc.

Optional:

- **Public URL** — when set, the app also configures vocalcord's
  per-account webhook delivery (lower-latency than WS). Defaults to
  WS-only.

API keys are stored plaintext in `tauri-plugin-store` for v1.

## Approvals over chat

When the running agent pauses for tool approval, the tray icon swaps
to `tray-attention` and the Status page surfaces the pending tool.
The user's next inbound message is interpreted:

- `approve` / `allow` / `yes` / `ok` / `lgtm` / `✅` → approve
- `deny` / `no` / `stop` / `cancel` / `❌` → deny
- anything else → deny the tool, then feed the text into the agent
  via `say(interrupt=true)` so the agent can rethink with the new
  context.

For questions (rather than approvals), the body is passed through as
the answer directly.

## Building distributable bundles

The current `icons/` directory has placeholder PNGs. Before
`npx tauri build`, run `npx tauri icon path/to/logo.png` to generate
proper bundle icons (`.icns` for macOS, `.ico` for Windows, multi-
resolution PNGs).
