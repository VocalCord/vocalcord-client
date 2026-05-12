import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface InboxItem {
  id: string;
  channel: string;
  from: string;
  from_display: string | null;
  preview: string;
  ts: string;
}

export interface EventLogEntry {
  ts: string;
  message: string;
}

export type ConnectionStatus =
  | { kind: 'connected' }
  | { kind: 'reconnecting' }
  | { kind: 'paused' };

export type AgentStatus =
  | { kind: 'stopped' }
  | { kind: 'running'; session_id: string }
  | { kind: 'waiting-for-approval'; approval_id: string; tool_name: string }
  | { kind: 'waiting-for-answer'; approval_id: string; tool_name: string }
  | { kind: 'error'; message: string };

export interface AppSnapshot {
  connection: ConnectionStatus;
  agent: AgentStatus;
  now_doing: string;
  inbox_preview: InboxItem[];
  event_log: EventLogEntry[];
}

export interface Settings {
  api_key: string;
  api_base: string;
  ws_base: string;
  public_url: string | null;
  webhook_port: number;
  webhook_path: string;
  agent_type: string;
  working_directory: string;
  permission_mode: string;
  approval_mode: string;
  reply_truncate_chars: number;
}

export const ipc = {
  getSnapshot: () => invoke<AppSnapshot>('get_snapshot'),
  getSettings: () => invoke<Settings>('get_settings'),
  updateSettings: (next: Settings) => invoke<void>('update_settings', { next }),
  toggleInbound: () => invoke<void>('toggle_inbound'),
  toggleAgent: () => invoke<void>('toggle_agent'),
};

export function onAppEvent<T = unknown>(
  name:
    | 'snapshot'
    | 'settings-changed'
    | 'agent-status'
    | 'approval-pending'
    | 'connection-status'
    | 'inbound-arrived',
  handler: (payload: T) => void,
): Promise<UnlistenFn> {
  return listen<T>(name, (e) => handler(e.payload));
}
