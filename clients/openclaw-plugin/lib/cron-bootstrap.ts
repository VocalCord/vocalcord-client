// Idempotent cron registration. The job runs every 6 hours regardless
// of rate-limit state — the plugin's `before_tool_call` hook
// short-circuits the actual MCP call when the gate is closed, so the
// cron is a no-op during a Retry-After window without us having to
// mutate the schedule.

import type { OpenclawApi } from './types.js';

const CRON_NAME = 'vocalcord-pull-sync';
const CRON_EXPR = '0 */6 * * *';
const CRON_PROMPT =
  'Call the vocalcord GetMessages tool to sync pull-style channels (Discord, Matrix, Signal, Urbit). Do not reply unless there is something the user needs to know.';

export async function ensureCron(api: OpenclawApi): Promise<void> {
  const list = api.runtime?.listCronJobs;
  const add = api.runtime?.addCronJob;
  if (!add) {
    api.log.warn(
      'vocalcord-openclaw-plugin: runtime.addCronJob not available; configure manually via `openclaw cron add`',
    );
    return;
  }

  if (list) {
    const existing = await list();
    if (existing.some((j) => j.name === CRON_NAME)) {
      api.log.debug?.('vocalcord-openclaw-plugin: cron already present', { name: CRON_NAME });
      return;
    }
  }

  const { id } = await add({
    name: CRON_NAME,
    cron: CRON_EXPR,
    message: CRON_PROMPT,
    tools: ['vocalcord'],
    session: 'isolated',
  });
  api.log.info('vocalcord-openclaw-plugin: pull-sync cron registered', {
    id,
    name: CRON_NAME,
    cron: CRON_EXPR,
  });
}
