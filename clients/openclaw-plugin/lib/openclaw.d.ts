// Ambient shim for the OpenClaw plugin SDK. The actual types ship
// with the host's `openclaw` peer dep (which we mark optional —
// the plugin lazy-loads it at runtime). This declaration keeps
// `tsc --noEmit` happy in environments where the host isn't
// installed (e.g. CI without an OpenClaw gateway).

declare module 'openclaw/plugin-sdk/plugin-entry' {
  export function definePluginEntry(args: unknown): unknown;
}
