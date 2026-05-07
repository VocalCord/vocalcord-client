// Configures vocalcord's per-account webhook delivery.
//
// Flow:
//   1. Generate a 32-byte secret client-side (we own it; server
//      never mints plaintext on our behalf).
//   2. Bind the local receiver with that secret.
//   3. POST /v1/webhooks/configure with `{ url?, port?, path?, secret }`.
//      - If `publicUrl` is set, we tell vocalcord exactly where
//        to deliver: `<publicUrl><path>`.
//      - If `publicUrl` is omitted, we send `{ port, path }` and
//        vocalcord uses the source IP of this request as the host.
//   4. Server runs a verification handshake: POSTs `{type:"verification",challenge}`
//      signed with our secret to the URL. Receiver verifies HMAC,
//      echoes `{challenge}`. Server commits iff handshake succeeds.
//   5. On any failure → caller falls back to the WebSocket bridge.

import { webcrypto } from 'node:crypto';

import { Receiver } from '../index.js';
import type { PluginConfig } from './types.js';

interface WebhookConfigureResponse {
  ok: boolean;
  reason?: string;
}

export interface WebhookSetupResult {
  handle: Receiver;
}

function genSecret(): string {
  const bytes = new Uint8Array(32);
  webcrypto.getRandomValues(bytes);
  // base64url, no padding
  let s = Buffer.from(bytes).toString('base64');
  s = s.replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  return s;
}

export async function trySetupWebhook(
  cfg: PluginConfig,
): Promise<WebhookSetupResult | null> {
  const apiBase = (cfg.apiBase ?? 'https://api.vocalcord.io').replace(/\/$/, '');
  const port = cfg.webhookPort ?? 18790;
  const path = cfg.webhookPath ?? '/vocalcord/inbound';
  const gatewayBaseUrl = cfg.gatewayBaseUrl ?? 'http://127.0.0.1:18789';

  const secret = genSecret();

  // 1. Bind the receiver FIRST — vocalcord's verification ping
  //    arrives before the configure call returns.
  let handle: Receiver;
  try {
    handle = await Receiver.start({
      bindAddr: `0.0.0.0:${port}`,
      path,
      secret,
      gatewayBaseUrl,
      hookToken: cfg.hookToken,
    });
  } catch {
    return null;
  }

  // 2. Ask vocalcord to register the URL using our secret.
  const body: Record<string, unknown> = { port, path, secret };
  if (cfg.publicUrl) {
    body.url = `${cfg.publicUrl.replace(/\/$/, '')}${path}`;
  }

  let res: Response;
  try {
    res = await fetch(`${apiBase}/v1/webhooks/configure`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${cfg.apiKey}`,
      },
      body: JSON.stringify(body),
    });
  } catch {
    handle.stop();
    return null;
  }

  if (!res.ok) {
    handle.stop();
    return null;
  }

  const j = (await res.json()) as WebhookConfigureResponse;
  if (!j.ok) {
    handle.stop();
    return null;
  }
  return { handle };
}
