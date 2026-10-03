import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

interface Mission {
  id: string;
  label: string;
  interval_ms: number;
  to: string;
  body: string;
  enabled: boolean;
  kind?: string;
  last_fired_at: number;
}

const missionsList = () => invoke<Mission[]>('missions_list');
const missionsSave = (m: Mission[]) => invoke('missions_save', { missions: m });

function fmtInterval(ms: number): string {
  if (ms >= 3_600_000) return `${(ms / 3_600_000).toFixed(ms % 3_600_000 ? 1 : 0)}h`;
  if (ms >= 60_000) return `${Math.round(ms / 60_000)}m`;
  return `${Math.round(ms / 1000)}s`;
}
function fmtAgo(ts: number): string {
  if (!ts) return 'never';
  const s = Math.floor((Date.now() - ts) / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}

/** The Schedules surface — the toggle the missions can't live without
 *  (upstream's Command Center Schedules tab, in miniature). */
export function MissionsPanel({ onClose }: { onClose: () => void }) {
  const [missions, setMissions] = useState<Mission[]>([]);

  const refresh = () => missionsList().then(setMissions);
  useEffect(() => {
    refresh();
    const un = listen('missions-updated', refresh);
    return () => {
      un.then((f) => f());
    };
  }, []);

  const toggle = async (m: Mission) => {
    const next = missions.map((x) => (x.id === m.id ? { ...x, enabled: !x.enabled } : x));
    setMissions(next);
    await missionsSave(next);
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Schedules</h2>
        {missions.map((m) => (
          <div key={m.id} className="mission-row">
            <div className="grow">
              <div className="agent-name">
                {m.label} {m.kind && <span className="tag">{m.kind}</span>}
              </div>
              <div className="agent-sub">
                every {fmtInterval(m.interval_ms)} · last {fmtAgo(m.last_fired_at)}
                {m.to ? ` · → ${m.to}` : ''}
              </div>
            </div>
            <label className="row">
              <input type="checkbox" checked={m.enabled} onChange={() => toggle(m)} />
              <span>{m.enabled ? 'on' : 'off'}</span>
            </label>
          </div>
        ))}
        {missions.length === 0 && <p className="empty">No missions configured.</p>}
        <div className="row end">
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
