import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

const list = () => invoke<string[]>('secrets_list');
const setSecret = (key: string, value: string) => invoke('secret_set', { key, value });
const del = (key: string) => invoke('secret_delete', { key });

/** Secrets surface — env keys (ANTHROPIC_API_KEY, OPENAI_API_KEY, …) stored
 *  in the OS credential store and injected into every spawned agent's env.
 *  Values are write-only: the frontend never sees them back. */
export function SecretsPanel({ onClose }: { onClose: () => void }) {
  const [keys, setKeys] = useState<string[]>([]);
  const [name, setName] = useState('');
  const [value, setValue] = useState('');

  const refresh = () => list().then(setKeys);
  useEffect(() => {
    refresh();
  }, []);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Secrets</h2>
        <p className="agent-sub">
          Stored in the OS keychain, injected as env into every spawned agent. Values are never displayed back.
        </p>
        {keys.map((k) => (
          <div key={k} className="mission-row">
            <div className="grow">
              <code>{k}</code>
            </div>
            <button className="ghost danger" onClick={async () => { await del(k); refresh(); }}>
              remove
            </button>
          </div>
        ))}
        {keys.length === 0 && <p className="empty">No secrets — add one below.</p>}
        <div className="row">
          <input placeholder="ANTHROPIC_API_KEY" value={name} onChange={(e) => setName(e.target.value)} />
          <input
            type="password"
            placeholder="value (write-only)"
            value={value}
            onChange={(e) => setValue(e.target.value)}
          />
          <button
            className="primary"
            onClick={async () => {
              await setSecret(name, value);
              setName('');
              setValue('');
              refresh();
            }}
          >
            Add
          </button>
        </div>
        <div className="row end">
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
