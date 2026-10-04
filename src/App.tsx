import { useEffect, useState } from 'react';
import { getConfig, onAgentStatus, onBreakerState, onPtyExit, setAutoMode } from './api';
import { AddAgent } from './components/AddAgent';
import { AgentList } from './components/AgentList';
import { ControlStrip } from './components/ControlStrip';
import { BoardPanel } from './components/BoardPanel';
import { InboxPanel } from './components/InboxPanel';
import { MissionsPanel } from './components/MissionsPanel';
import { OfficeFloor } from './scene/office/OfficeFloor';
import { SecretsPanel } from './components/SecretsPanel';
import { SkillsPanel } from './components/SkillsPanel';
import { TriggersPanel } from './components/TriggersPanel';
import { TerminalPane } from './components/TerminalPane';
import { useApp } from './store';

export default function App() {
  const { agents, selected, refreshAgents, setBreaker, select } = useApp();
  const [adding, setAdding] = useState(false);
  const [showMissions, setShowMissions] = useState(false);
  const [showBoard, setShowBoard] = useState(false);
  const [showSecrets, setShowSecrets] = useState(false);
  const [showTriggers, setShowTriggers] = useState(false);
  const [showSkills, setShowSkills] = useState(false);
  const [autoMode, setAuto] = useState(false);

  useEffect(() => {
    // Floor → app selection bridge: clicking an avatar in the Pixi scene
    // opens its terminal; the task boards open the Board panel.
    const onSelect = (ev: Event) => {
      const id = (ev as CustomEvent<{ id?: string }>).detail?.id;
      if (id) select(id);
    };
    const onTab = (ev: Event) => {
      const tab = (ev as CustomEvent<{ tab?: string }>).detail?.tab;
      if (tab === 'tasks' || tab === 'board') setShowBoard(true);
    };
    window.addEventListener('ao:select-agent', onSelect);
    window.addEventListener('ao:command-center-tab', onTab);
    refreshAgents();
    getConfig().then((c) => setAuto(c.auto_mode));
    const un = onPtyExit(() => refreshAgents());
    const unStatus = onAgentStatus(() => refreshAgents());
    const unBreaker = onBreakerState((b) => setBreaker(b));
    const poll = setInterval(refreshAgents, 5000);
    return () => {
      window.removeEventListener('ao:select-agent', onSelect);
      window.removeEventListener('ao:command-center-tab', onTab);
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
          <span>
            {agent && (
              <button className="ghost" onClick={() => select(null)}>
                ← floor
              </button>
            )}{' '}
            {agent ? `${agent.name} — ${agent.provider}` : 'Office floor'}
          </span>
          <button className="ghost" onClick={() => setShowBoard(true)}>
            Board
          </button>
          <button className="ghost" onClick={() => setShowMissions(true)}>
            Schedules
          </button>
          <button className="ghost" onClick={() => setShowTriggers(true)}>
            Triggers
          </button>
          <button className="ghost" onClick={() => setShowSkills(true)}>
            Skills
          </button>
          <button className="ghost" onClick={() => setShowSecrets(true)}>
            Secrets
          </button>
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
          <OfficeFloor />
        )}
      </main>
      {showBoard && <BoardPanel onClose={() => setShowBoard(false)} />}
      {showMissions && <MissionsPanel onClose={() => setShowMissions(false)} />}
      {showSecrets && <SecretsPanel onClose={() => setShowSecrets(false)} />}
      {showTriggers && <TriggersPanel onClose={() => setShowTriggers(false)} />}
      {showSkills && <SkillsPanel onClose={() => setShowSkills(false)} />}
      {adding && <AddAgent onClose={() => setAdding(false)} />}
    </div>
  );
}
