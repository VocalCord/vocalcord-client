import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

import { SettingsPage } from './SettingsPage';
import { StatusPage } from './StatusPage';

type Tab = 'status' | 'settings';

export function App() {
  const [tab, setTab] = useState<Tab>('status');

  // The tray menu's "Settings" item emits `nav` so we can pop the
  // user straight into the right tab without an extra click.
  useEffect(() => {
    const unsub = listen<string>('nav', (e) => {
      if (e.payload === 'settings') setTab('settings');
      if (e.payload === 'status') setTab('status');
    });
    return () => {
      void unsub.then((fn) => fn());
    };
  }, []);

  return (
    <div className="app">
      <nav className="tabs">
        <button
          className={tab === 'status' ? 'active' : ''}
          onClick={() => setTab('status')}
        >
          Status
        </button>
        <button
          className={tab === 'settings' ? 'active' : ''}
          onClick={() => setTab('settings')}
        >
          Settings
        </button>
      </nav>
      <main>{tab === 'status' ? <StatusPage /> : <SettingsPage />}</main>
    </div>
  );
}
