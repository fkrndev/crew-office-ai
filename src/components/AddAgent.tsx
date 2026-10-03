import { useState } from 'react';
import { getConfig, spawnAgent } from '../api';
import {
  AGENT_PROVIDER_PRESETS,
  buildSpawnCommand,
  providerPreset,
  type AgentProvider
} from '../providers';
import { useApp } from '../store';

export function AddAgent({ onClose }: { onClose: () => void }) {
  const [provider, setProvider] = useState<AgentProvider>('claude');
  const [name, setName] = useState('');
  const [role, setRole] = useState('');
  const [cwd, setCwd] = useState('');
  const [model, setModel] = useState('');
  const [command, setCommand] = useState('');
  const [isGod, setIsGod] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const refreshAgents = useApp((s) => s.refreshAgents);

  const preset = providerPreset(provider);

  const submit = async () => {
    setBusy(true);
    setError('');
    try {
      const cfg = await getConfig();
      const cmd = buildSpawnCommand(provider, command, cfg.auto_mode, model || undefined);
      await spawnAgent({
        provider,
        name: name || preset.label,
        role,
        cwd,
        command: cmd,
        is_god: isGod
      });
      await refreshAgents();
      onClose();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Hire an agent</h2>
        <label>
          Engine
          <select value={provider} onChange={(e) => setProvider(e.target.value as AgentProvider)}>
            {AGENT_PROVIDER_PRESETS.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
              </option>
            ))}
          </select>
        </label>
        <label>
          Name
          <input value={name} onChange={(e) => setName(e.target.value)} placeholder={preset.label} />
        </label>
        <label>
          Role
          <input value={role} onChange={(e) => setRole(e.target.value)} placeholder="researcher, coder, …" />
        </label>
        <label>
          Working directory
          <input value={cwd} onChange={(e) => setCwd(e.target.value)} placeholder="(agent folder)" />
        </label>
        {preset.supportsModel && (
          <label>
            Model
            <input
              value={model}
              onChange={(e) => setModel(e.target.value)}
              placeholder={preset.recommendedOrchestratorModel ?? 'default'}
            />
          </label>
        )}
        <label>
          Command override
          <input
            value={command}
            onChange={(e) => setCommand(e.target.value)}
            placeholder={preset.defaultCommand || 'command line'}
          />
        </label>
        <label className="row">
          <input type="checkbox" checked={isGod} onChange={(e) => setIsGod(e.target.checked)} />
          Orchestrator (god)
        </label>
        {error && <div className="error">{error}</div>}
        <div className="row end">
          <button onClick={onClose}>Cancel</button>
          <button className="primary" disabled={busy} onClick={submit}>
            {busy ? 'Spawning…' : 'Hire'}
          </button>
        </div>
      </div>
    </div>
  );
}
