import { useEffect, useState } from 'react';
import {
  appendMemory,
  hiveInbox,
  hiveMarkDone,
  hiveSend,
  onDeliver,
  readMemory
} from '../api';
import { useApp } from '../store';

/** Inbox + memory view for the selected agent — the hive surface of
 *  CommandCenter's Threads/Memory tabs, minus the visuals. */
export function InboxPanel({ agentId }: { agentId: string }) {
  const [tab, setTab] = useState<'inbox' | 'memory'>('inbox');
  const [memory, setMemory] = useState('');
  const [memoEntry, setMemoEntry] = useState('');
  const [to, setTo] = useState('god');
  const [subject, setSubject] = useState('');
  const [body, setBody] = useState('');
  const { inbox, setInbox, agents } = useApp();
  const msgs = inbox[agentId] ?? [];

  const refresh = async () => setInbox(agentId, await hiveInbox(agentId));

  useEffect(() => {
    refresh();
    const un = onDeliver((d) => {
      if (d.to === agentId) refresh();
    });
    return () => {
      un.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [agentId]);

  useEffect(() => {
    if (tab === 'memory') readMemory(agentId).then(setMemory);
  }, [tab, agentId]);

  return (
    <div className="inbox-panel">
      <div className="tabs">
        <button className={tab === 'inbox' ? 'active' : ''} onClick={() => setTab('inbox')}>
          Inbox ({msgs.length})
        </button>
        <button className={tab === 'memory' ? 'active' : ''} onClick={() => setTab('memory')}>
          Memory
        </button>
      </div>

      {tab === 'inbox' && (
        <>
          <ul className="msg-list">
            {msgs.map((m) => (
              <li key={m.id} className="msg">
                <div className="msg-head">
                  <span className="tag">{m.act}</span>
                  <strong>{m.subject || '(no subject)'}</strong>
                  <span className="msg-from">from {m.from}</span>
                  <button className="ghost" onClick={() => hiveMarkDone(agentId, m.id).then(refresh)}>
                    done
                  </button>
                </div>
                <pre>{m.body}</pre>
              </li>
            ))}
            {msgs.length === 0 && <li className="empty">Inbox empty.</li>}
          </ul>
          <div className="compose">
            <div className="row">
              <select value={to} onChange={(e) => setTo(e.target.value)}>
                <option value="god">god</option>
                <option value="broadcast">broadcast</option>
                {agents
                  .filter((a) => a.id !== agentId)
                  .map((a) => (
                    <option key={a.id} value={a.id}>
                      {a.name}
                    </option>
                  ))}
              </select>
              <input placeholder="subject" value={subject} onChange={(e) => setSubject(e.target.value)} />
            </div>
            <textarea placeholder="message body" value={body} onChange={(e) => setBody(e.target.value)} />
            <button
              className="primary"
              onClick={async () => {
                await hiveSend(agentId, to, 'request', subject, body);
                setSubject('');
                setBody('');
              }}
            >
              Send
            </button>
          </div>
        </>
      )}

      {tab === 'memory' && (
        <div className="memory">
          <pre className="memory-view">{memory}</pre>
          <div className="row">
            <input
              placeholder="append a memory entry…"
              value={memoEntry}
              onChange={(e) => setMemoEntry(e.target.value)}
            />
            <button
              onClick={async () => {
                await appendMemory(agentId, memoEntry);
                setMemoEntry('');
                setMemory(await readMemory(agentId));
              }}
            >
              Append
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
