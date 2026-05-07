// Tool-call hooks honoring 429 + Retry-After.
//
// Design (per plan): the 6-hour pull-sync cron always fires. When
// vocalcord 429s, the after-hook records the Retry-After window in
// the in-memory rate-limit gate. Every subsequent
// `vocalcord.GetMessages` call (cron or user-driven) hits the
// before-hook first; if the gate is closed, the hook short-circuits
// the call with a synthetic prose result. NO in-process retry, NO
// cron mutation.

import type { RateLimitGate } from '../index.js';
import type { HookHandlerResult, ToolCallEvent } from './types.js';

const TARGET_TOOL = 'vocalcord.GetMessages';

export function makeBeforeToolCall(gate: RateLimitGate) {
  return (event: ToolCallEvent): HookHandlerResult => {
    if (event.tool.name !== TARGET_TOOL) return;
    if (!gate.isBlocked()) return;
    const remainingMs = gate.remainingMs() ?? 0;
    const minutes = Math.ceil(remainingMs / 60_000);
    return {
      decision: 'short-circuit',
      result: `Vocal Cord is rate-limited; skipping GetMessages for ~${minutes} more minutes.`,
    };
  };
}

export function makeAfterToolCall(gate: RateLimitGate) {
  return (event: ToolCallEvent): HookHandlerResult => {
    if (event.tool.name !== TARGET_TOOL) return;
    const ra = parseRetryAfter(event);
    if (ra && ra > 0) {
      gate.record429(ra);
    }
  };
}

function parseRetryAfter(event: ToolCallEvent): number | null {
  // Server-side contract (see crates/vocalcord-mcp-server/src/handler.rs):
  // a rate-limit-exceeded error surfaces `retryAfterSec` in
  // `error.data`. Detection is purely by presence of that field —
  // robust to JSON-RPC code remapping or message-format tweaks.
  const data = event.error?.data;
  if (data && typeof data === 'object') {
    const v = (data as Record<string, unknown>).retryAfterSec;
    if (typeof v === 'number' && Number.isFinite(v) && v > 0) return v;
  }
  return null;
}
