import { useEffect, useState } from 'react';

import { ipc, onAppEvent, type AppSnapshot } from './ipc';

export function StatusPage() {
  const [snap, setSnap] = useState<AppSnapshot | null>(null);

  useEffect(() => {
    void ipc.getSnapshot().then(setSnap);
    const unsub = onAppEvent<AppSnapshot>('snapshot', setSnap);
    return () => {
      void unsub.then((fn) => fn());
    };
  }, []);

  if (!snap) return <div className="loading">Loading…</div>;

  return (
    <div className="status-page">
      <header className="chips">
        <ConnectionChip status={snap.connection} />
        <AgentChip status={snap.agent} />
        <div className="actions">
          <button onClick={() => ipc.toggleAgent()}>
            {snap.agent.kind === 'stopped' ? 'Start agent' : 'Stop agent'}
          </button>
          <button onClick={() => ipc.toggleInbound()}>
            {snap.connection.kind === 'paused' ? 'Resume inbound' : 'Pause inbound'}
          </button>
        </div>
      </header>

      <section className="now">
        <h2>Now</h2>
        <p>{snap.now_doing || 'Idle'}</p>
      </section>

      <section className="inbox">
        <h2>Inbox</h2>
        {snap.inbox_preview.length === 0 ? (
          <p className="empty">No recent messages.</p>
        ) : (
          <ul>
            {snap.inbox_preview.map((m) => (
              <li key={m.id}>
                <span className="channel">{m.channel}</span>
                <span className="from">{m.from_display ?? m.from}</span>
                <span className="preview">{m.preview}</span>
                <span className="ts">{relativeTs(m.ts)}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="events">
        <h2>Events</h2>
        <ul>
          {snap.event_log
            .slice()
            .reverse()
            .map((e, i) => (
              <li key={i}>
                <span className="ts">{relativeTs(e.ts)}</span>
                <span className="msg">{e.message}</span>
              </li>
            ))}
        </ul>
      </section>
    </div>
  );
}

function ConnectionChip({ status }: { status: AppSnapshot['connection'] }) {
  const label =
    status.kind === 'connected'
      ? 'Connected'
      : status.kind === 'reconnecting'
        ? 'Reconnecting…'
        : 'Paused';
  return <span className={`chip conn-${status.kind}`}>{label}</span>;
}

function AgentChip({ status }: { status: AppSnapshot['agent'] }) {
  let label = 'Stopped';
  if (status.kind === 'running') label = 'Running';
  if (status.kind === 'waiting-for-approval') label = `Awaiting approval: ${status.tool_name}`;
  if (status.kind === 'waiting-for-answer') label = `Awaiting answer: ${status.tool_name}`;
  if (status.kind === 'error') label = `Error: ${status.message}`;
  return <span className={`chip agent-${status.kind}`}>{label}</span>;
}

function relativeTs(iso: string): string {
  try {
    const t = new Date(iso).getTime();
    if (!Number.isFinite(t)) return iso;
    const delta = (Date.now() - t) / 1000;
    if (delta < 60) return 'just now';
    if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
    if (delta < 86400) return `${Math.floor(delta / 3600)}h ago`;
    return `${Math.floor(delta / 86400)}d ago`;
  } catch {
    return iso;
  }
}
