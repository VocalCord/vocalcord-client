# Vocal Cord desktop app (placeholder)

Coming soon. This will be a Tauri-based desktop app that delivers
inbound Vocal Cord messages as native OS notifications and lets the
user trigger local AI agents in response.

## Architecture preview

The desktop app will add `push-core` to its
`src-tauri/Cargo.toml` and call its `Receiver::start` directly —
no napi, no FFI hop — reusing the exact same HMAC verifier,
in-memory rate-limit gate, and dedup LRU as the OpenClaw plugin.

When this directory grows real code, the workspace `Cargo.toml`
gets `clients/desktop/src-tauri` added to its `members` list.
