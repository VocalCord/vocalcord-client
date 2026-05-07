// Vocal Cord OpenClaw plugin entry.
//
// Lifecycle (gateway_start):
//   1. Validate config; warn-and-noop if apiKey or hookToken missing.
//   2. Register the MCP server in OpenClaw's config (idempotent).
//   3. Try the webhook delivery path. On any failure, fall back to
//      a long-lived WebSocket bridge.
//   4. Register the 6-hourly pull-sync cron (idempotent).
//   5. Wire `before_tool_call` / `after_tool_call` rate-limit hooks
//      on `vocalcord.GetMessages`.
//
// Lifecycle (gateway_stop): close all handles.

import { RateLimitGate } from '../index.js';
import { ensureCron } from './cron-bootstrap.js';
import { ensureMcpServer } from './mcp-config.js';
import { makeAfterToolCall, makeBeforeToolCall } from './rl-hooks.js';
import type { OpenclawApi, PluginConfig, DefinePluginEntryArgs } from './types.js';
import { trySetupWebhook, type WebhookSetupResult } from './webhook-setup.js';
import { startWsBridge, type WsBridgeHandle } from './ws-bridge.js';

// `definePluginEntry` from `openclaw/plugin-sdk/plugin-entry` is a
// peer dep we don't import directly — see types.ts. We forward to
// the host's definePluginEntry at module-load time.
async function loadDefiner(): Promise<(args: DefinePluginEntryArgs) => unknown> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const mod: any = await import('openclaw/plugin-sdk/plugin-entry').catch(() => null);
  if (mod?.definePluginEntry) return mod.definePluginEntry;
  // Test / no-host fallback: just hand back the args object so
  // unit tests can exercise the registration shape.
  return (args: DefinePluginEntryArgs) => args;
}

const definePluginEntry = await loadDefiner();

export default definePluginEntry({
  id: 'vocalcord-openclaw-plugin',
  name: 'Vocal Cord',
  async register(api: OpenclawApi) {
    const cfg = api.config<PluginConfig>();
    if (!cfg.apiKey || !cfg.hookToken) {
      api.log.warn(
        'vocalcord-openclaw-plugin: apiKey or hookToken not configured; plugin will not start',
      );
      return;
    }

    let webhook: WebhookSetupResult | null = null;
    let ws: WsBridgeHandle | null = null;
    const gate = new RateLimitGate();

    api.on('gateway_start', async () => {
      await ensureMcpServer(api, cfg.apiBase ?? 'https://api.vocalcord.io', cfg.apiKey);
      await ensureCron(api);

      webhook = await trySetupWebhook(cfg);
      if (webhook) {
        api.log.info('vocalcord-openclaw-plugin: webhook receiver active', {
          localAddr: webhook.handle.localAddr,
        });
      } else {
        api.log.info('vocalcord-openclaw-plugin: webhook unavailable; falling back to WebSocket');
        ws = startWsBridge(api, cfg);
      }
    });

    api.on('before_tool_call', makeBeforeToolCall(gate), { priority: 100 });
    api.on('after_tool_call', makeAfterToolCall(gate), { priority: 100 });

    api.on('gateway_stop', () => {
      try {
        webhook?.handle.stop();
      } catch {
        /* shutdown best-effort */
      }
      try {
        ws?.stop();
      } catch {
        /* shutdown best-effort */
      }
    });
  },
});
