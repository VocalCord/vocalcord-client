import { useEffect, useState } from 'react';

import { ipc, type Settings } from './ipc';

const AGENT_TYPES = [
  'claude-code',
  'gemini-cli',
  'codex',
  'amp',
  'aider',
  'cursor',
  'opencode',
  'qwen-code',
  'copilot',
  'droid',
];

const PERMISSION_MODES = ['auto', 'supervised', 'plan'];
const APPROVAL_MODES = ['allow', 'ask', 'elicitation'];

export function SettingsPage() {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  useEffect(() => {
    void ipc.getSettings().then(setSettings);
  }, []);

  if (!settings) return <div className="loading">Loading…</div>;

  const onSave = async () => {
    setSaving(true);
    try {
      await ipc.updateSettings(settings);
      setSaved(true);
      setTimeout(() => setSaved(false), 1800);
    } finally {
      setSaving(false);
    }
  };

  const set = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    setSettings({ ...settings, [key]: value });

  return (
    <div className="settings-page">
      <h2>Vocal Cord</h2>
      <Field label="API key (account UUID)">
        <input
          type="password"
          value={settings.api_key}
          onChange={(e) => set('api_key', e.target.value)}
          placeholder="00000000-0000-0000-0000-000000000000"
        />
      </Field>
      <Field label="API base">
        <input
          type="text"
          value={settings.api_base}
          onChange={(e) => set('api_base', e.target.value)}
        />
      </Field>
      <Field label="WebSocket base">
        <input
          type="text"
          value={settings.ws_base}
          onChange={(e) => set('ws_base', e.target.value)}
        />
      </Field>
      <Field label="Public URL (optional — enables webhook path)">
        <input
          type="text"
          value={settings.public_url ?? ''}
          onChange={(e) => set('public_url', e.target.value || null)}
          placeholder="https://my-host.tailnet.ts.net"
        />
      </Field>
      <Field label="Webhook port">
        <input
          type="number"
          value={settings.webhook_port}
          onChange={(e) => set('webhook_port', Number(e.target.value) || 18790)}
        />
      </Field>

      <h2>Coding agent</h2>
      <Field label="Agent type">
        <select
          value={settings.agent_type}
          onChange={(e) => set('agent_type', e.target.value)}
        >
          {AGENT_TYPES.map((a) => (
            <option key={a} value={a}>
              {a}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Working directory">
        <input
          type="text"
          value={settings.working_directory}
          onChange={(e) => set('working_directory', e.target.value)}
        />
      </Field>
      <Field label="Permission mode">
        <select
          value={settings.permission_mode}
          onChange={(e) => set('permission_mode', e.target.value)}
        >
          {PERMISSION_MODES.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Approval mode">
        <select
          value={settings.approval_mode}
          onChange={(e) => set('approval_mode', e.target.value)}
        >
          {APPROVAL_MODES.map((m) => (
            <option key={m} value={m}>
              {m}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Reply truncate (chars)">
        <input
          type="number"
          value={settings.reply_truncate_chars}
          onChange={(e) => set('reply_truncate_chars', Number(e.target.value) || 2048)}
        />
      </Field>

      <div className="actions">
        <button disabled={saving} onClick={onSave}>
          {saving ? 'Saving…' : saved ? 'Saved' : 'Save'}
        </button>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="field">
      <span>{label}</span>
      {children}
    </label>
  );
}
