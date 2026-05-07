// WebSocket fallback bridge.
//
// Used when the webhook setup path fails (no public reachability,
// free-tier account, server returned 409, etc.). Opens a long-lived
// WS to vocalcord via the official SDK and forwards every inbound
// `MessageEvent` into the local OpenClaw Gateway via the napi
// `wake` helper (which itself routes through `push-core`'s
// `HooksClient`, so the desktop app's path stays identical).

import { VocalCordClient, type MessageEvent } from '@vocalcord/sdk';

import { wake } from '../index.js';
import type { OpenclawApi, PluginConfig } from './types.js';

export interface WsBridgeHandle {
  stop(): void;
}

function buildWakeText(msg: MessageEvent): string {
  const fromLabel = msg.from_display ?? msg.from;
  const preview = truncate(msg.body, 280);
  if (msg.subject) {
    return `New ${msg.channel} from ${fromLabel}: ${msg.subject} — ${preview}`;
  }
  return `New ${msg.channel} from ${fromLabel}: ${preview}`;
}

function truncate(s: string, max: number): string {
  if ([...s].length <= max) return s;
  return [...s].slice(0, max).join('') + '…';
}

export function startWsBridge(api: OpenclawApi, cfg: PluginConfig): WsBridgeHandle {
  const gatewayBaseUrl = cfg.gatewayBaseUrl ?? 'http://127.0.0.1:18789';
  const client = new VocalCordClient({
    apiKey: cfg.apiKey,
    apiBase: cfg.apiBase,
    wsBase: cfg.wsBase,
    clientId: 'openclaw-plugin',
  });

  client.on('message', (msg: MessageEvent) => {
    void wake(gatewayBaseUrl, cfg.hookToken, buildWakeText(msg)).catch(
      (e: unknown) => api.log.warn('vocalcord-openclaw-plugin: wake() failed', { error: String(e) }),
    );
  });
  client.on('error', (err: unknown) =>
    api.log.warn('vocalcord-openclaw-plugin: ws error', { error: String(err) }),
  );

  api.log.info('vocalcord-openclaw-plugin: WebSocket bridge active');

  return {
    stop() {
      client.close();
    },
  };
}
