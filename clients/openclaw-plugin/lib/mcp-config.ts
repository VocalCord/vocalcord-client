// Idempotent MCP server registration — writes (or refreshes) the
// `mcp.servers.vocalcord` block in OpenClaw's config.

import type { OpenclawApi } from './types.js';

export async function ensureMcpServer(
  api: OpenclawApi,
  apiBase: string,
  apiKey: string,
): Promise<void> {
  const url = `${apiBase.replace(/\/$/, '')}/mcp`;
  const cfg = {
    url,
    transport: 'streamable-http' as const,
    headers: { Authorization: `Bearer ${apiKey}` },
  };

  const setter = api.runtime?.setMcpServer;
  if (!setter) {
    api.log.warn(
      'vocalcord-openclaw-plugin: runtime.setMcpServer not available; configure manually via `openclaw mcp set vocalcord ...`',
      { url },
    );
    return;
  }
  await setter('vocalcord', cfg);
  api.log.info('vocalcord-openclaw-plugin: MCP server registered', { url });
}
