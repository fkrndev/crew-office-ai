import { killAgent, respawnAgent } from '../api';
import { useApp } from '../store';

export function AgentList({ onAdd }: { onAdd: () => void }) {
  const { agents, selected, select, refreshAgents } = useApp();

  return (
    <aside className="sidebar">
      <div className="sidebar-head">
        <h1>Office</h1>
        <button className="primary" onClick={onAdd}>
          + Hire
        </button>
      </div>
      <ul className="agent-list">
        {agents.map((a) => (
          <li
            key={a.id}
            className={a.id === selected ? 'selected' : ''}
            onClick={() => select(a.id)}
          >
            <div className="agent-row">
              <span className={`dot ${a.status}`} />
              <div>
                <div className="agent-name">
                  {a.name} {a.is_god && <span className="tag">god</span>}
                </div>
                <div className="agent-sub">
                  {a.provider} · {a.status || 'unknown'}
                </div>
              </div>
              {a.status === 'stopped' && (
                <button
                  className="ghost"
                  title="Respawn — same id, same hive folder (claude resumes its session)"
                  onClick={async (e) => {
                    e.stopPropagation();
                    await respawnAgent(a.id).catch((err) => alert(String(err)));
                    await refreshAgents();
                  }}
                >
                  ↻
                </button>
              )}
              <button
                className="ghost danger"
                title="Stop agent"
                onClick={async (e) => {
                  e.stopPropagation();
                  await killAgent(a.id);
                  await refreshAgents();
                }}
              >
                ×
              </button>
            </div>
          </li>
        ))}
        {agents.length === 0 && <li className="empty">No agents yet — hire one.</li>}
      </ul>
    </aside>
  );
}
