import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

interface TriggerEntry {
  id: string;
  source: string;
  sourceId: string;
  sourceName: string;
  direction: string;
  peer: string;
  title?: string;
  body: string;
  kind: string;
  decision?: string;
  correlationId?: string;
  taskId?: string;
  at: number;
}

const list = () => invoke<TriggerEntry[]>('triggers_list');
const clear = (source?: string) => invoke('triggers_clear', { source: source ?? null });
const pending = () => invoke<[string, { message?: string; title?: string; from?: string }][]>('webhook_pending');
const approve = (token: string) => invoke('webhook_approve', { token });
const reject = (token: string) => invoke('webhook_reject', { token });

function fmtAgo(ts: number): string {
  if (!ts) return '';
  const s = Math.floor((Date.now() - ts) / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}

const DECISION_COLOR: Record<string, string> = {
  'auto-allowed': '#4ade80',
  pending: '#fbbf24',
  approved: '#4ade80',
  rejected: '#f87171',
};

/** The Triggers tab — the inbound trigger ledger (webhook arrivals + the
 *  operator's verdicts) plus the pending-review queue with approve/reject. */
export function TriggersPanel({ onClose }: { onClose: () => void }) {
  const [entries, setEntries] = useState<TriggerEntry[]>([]);
  const [queue, setQueue] = useState<[string, { message?: string; title?: string; from?: string }][]>([]);
  const [expanded, setExpanded] = useState<string | null>(null);

  const refresh = () => {
    list().then(setEntries);
    pending().then(setQueue);
  };
  useEffect(() => {
    refresh();
    const un1 = listen('webhook-pending', refresh);
    const un2 = listen('hook-event', refresh);
    const t = setInterval(refresh, 5000);
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      clearInterval(t);
    };
  }, []);

  const doApprove = async (token: string) => {
    await approve(token);
    refresh();
  };
  const doReject = async (token: string) => {
    await reject(token);
    refresh();
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <h2>Triggers</h2>
        {queue.length > 0 && (
          <>
            <h3>Pending review ({queue.length})</h3>
            {queue.map(([token, body]) => (
              <div key={token} className="mission-row">
                <div className="grow">
                  <div className="agent-name">{body.title || 'webhook'}</div>
                  <div className="agent-sub">
                    {body.from ? `from ${body.from} · ` : ''}
                    {(body.message || '').slice(0, 140)}
                  </div>
                </div>
                <button onClick={() => doApprove(token)}>Approve</button>
                <button className="ghost" onClick={() => doReject(token)}>
                  Reject
                </button>
              </div>
            ))}
          </>
        )}
        <div className="row" style={{ justifyContent: 'space-between' }}>
          <h3>History ({entries.length})</h3>
          <button className="ghost" onClick={() => clear('webhook').then(refresh)}>
            Clear webhook
          </button>
        </div>
        {entries.slice(0, 60).map((e) => (
          <div key={e.id} className="mission-row" onClick={() => setExpanded(expanded === e.id ? null : e.id)} style={{ cursor: 'pointer' }}>
            <div className="grow">
              <div className="agent-name">
                {e.direction === 'inbound' ? '←' : '→'} {e.title || e.sourceName || e.source}{' '}
                {e.kind && <span className="tag">{e.kind}</span>}{' '}
                {e.decision && (
                  <span className="tag" style={{ color: DECISION_COLOR[e.decision] || '#aaa' }}>
                    {e.decision}
                  </span>
                )}
              </div>
              <div className="agent-sub">
                {e.peer ? `${e.peer} · ` : ''}
                {fmtAgo(e.at)}
                {e.taskId ? ` · task ${e.taskId.slice(0, 8)}` : ''}
              </div>
              {expanded === e.id && <div className="agent-sub" style={{ whiteSpace: 'pre-wrap' }}>{e.body}</div>}
            </div>
          </div>
        ))}
        {entries.length === 0 && <p className="empty">No trigger history yet.</p>}
        <div className="row end">
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
