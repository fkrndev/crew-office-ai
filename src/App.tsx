import { useEffect, useState } from 'react';
import { getConfig, onAgentStatus, onBreakerState, onPtyExit, setAutoMode } from './api';
import { AddAgent } from './components/AddAgent';
import { AgentList } from './components/AgentList';
import { ControlStrip } from './components/ControlStrip';
import { InboxPanel } from './components/InboxPanel';
import { TerminalPane } from './components/TerminalPane';
import { useApp } from './store';

export default function App() {
  const { agents, selected, refreshAgents, setBreaker } = useApp();
  const [adding, setAdding] = useState(false);
  const [autoMode, setAuto] = useState(false);

  useEffect(() => {
    refreshAgents();
    getConfig().then((c) => setAuto(c.auto_mode));
    const un = onPtyExit(() => refreshAgents());
    const unStatus = onAgentStatus(() => refreshAgents());
    const unBreaker = onBreakerState((b) => setBreaker(b));
    const poll = setInterval(refreshAgents, 5000);
    return () => {
      un.then((f) => f());
      unStatus.then((f) => f());
      unBreaker.then((f) => f());
      clearInterval(poll);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const agent = agents.find((a) => a.id === selected);

  return (
    <div className="app">
      <AgentList onAdd={() => setAdding(true)} />
      <main>
        <header className="topbar">
          <span>{agent ? `${agent.name} — ${agent.provider}` : 'Office Agent'}</span>
          <label className="row">
            <input
              type="checkbox"
              checked={autoMode}
              onChange={async (e) => {
                setAuto(e.target.checked);
                await setAutoMode(e.target.checked);
              }}
            />
            auto mode
          </label>
        </header>
        {agent ? (
          <>
            <ControlStrip agentId={agent.id} />
            <div className="split">
              <TerminalPane agentId={agent.id} />
              <InboxPanel agentId={agent.id} />
            </div>
          </>
        ) : (
          <div className="welcome">
            <h2>No agent selected</h2>
            <p>Hire an agent to put it on the floor.</p>
          </div>
        )}
      </main>
      {adding && <AddAgent onClose={() => setAdding(false)} />}
    </div>
  );
}
