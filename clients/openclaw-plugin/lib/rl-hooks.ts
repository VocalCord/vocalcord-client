// Tool-call hooks honoring 429 + Retry-After.
//
// Design (per plan): the 6-hour pull-sync cron always fires. When
// vocalcord 429s, the after-hook records the Retry-After window in
// the in-memory rate-limit gate. Every subsequent
// `vocalcord.GetMessages` call (cron or user-driven) hits the
// before-hook first; if the gate is closed, the hook short-circuits
// the call with a synthetic prose result. NO in-process retry, NO
// cron mutation.

import type { RateLimitGate } from './binding.js';
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
  const code = event.error?.code;
  const data = event.error?.data;
  // Server-side contract (see vocalcord-lambda-mcp): 429 surfaces
  // as { code: 429 (or "RateLimited"), data: { retryAfterSec } }.
  const looksLike429 =
    code === 429 || code === '429' || code === 'RateLimited' || code === 'rate_limited';
  if (!looksLike429) return null;
  if (data && typeof data === 'object') {
    const v = (data as Record<string, unknown>).retryAfterSec;
    if (typeof v === 'number' && Number.isFinite(v)) return v;
  }
  return null;
}
