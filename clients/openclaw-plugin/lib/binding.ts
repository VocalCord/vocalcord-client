// Loader for the napi-rs prebuilt addon.
//
// At runtime this resolves to the platform-specific `.node` file
// shipped alongside the package (or downloaded via
// optionalDependencies on `npm install`). We keep the surface here
// hand-typed so the rest of the TS code can import normally; once
// `napi build --dts binding.d.ts` is wired into CI, replace this
// with the auto-generated declarations.

import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let binding: any;
try {
  binding = require('../binding.cjs');
} catch {
  // Local development: napi-rs writes the .node directly into the
  // package directory.
  binding = require('../vocalcord-openclaw-plugin.node');
}

export interface ReceiverOptions {
  bindAddr: string;
  path: string;
  /** Webhook secret (base64url-no-pad as returned by vocalcord). */
  secret: string;
  /** OpenClaw Gateway base URL (e.g. http://127.0.0.1:18789). */
  gatewayBaseUrl: string;
  /** OpenClaw hooks bearer token. */
  hookToken: string;
}

export interface ReceiverHandle {
  /** Local socket address the listener is bound to. */
  readonly localAddr: string;
  stop(): void;
}

export interface NativeReceiver {
  start(opts: ReceiverOptions): Promise<ReceiverHandle>;
}

export interface RateLimitGate {
  isBlocked(): boolean;
  record429(retryAfterSeconds: number): void;
  /** Remaining ms until the gate opens; null if not blocked. */
  remainingMs(): number | null;
  clear(): void;
}

export const Receiver = binding.Receiver as NativeReceiver;
export const RateLimitGate = binding.RateLimitGate as new () => RateLimitGate;
export const wake: (
  gatewayBaseUrl: string,
  hookToken: string,
  text: string,
) => Promise<void> = binding.wake;
