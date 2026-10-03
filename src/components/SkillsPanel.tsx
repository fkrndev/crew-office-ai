import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

interface LocalSkill {
  id: string;
  name: string;
  description: string;
  provider: string;
  scope: string;
  path: string;
}
interface CatalogSkill {
  name: string;
  description: string;
  url: string;
  category: string;
  owner: string;
}
interface CatalogResult {
  skills: CatalogSkill[];
  fetched_at: number;
  stale: boolean;
  error?: string;
}

const localList = () => invoke<LocalSkill[]>('skills_local', { cwds: [] });
const catalog = (force: boolean) => invoke<CatalogResult>('skills_catalog', { force });
const install = (url: string, name: string) => invoke<string>('skills_install', { url, name });
const uninstall = (path: string) => invoke<boolean>('skills_uninstall', { path, cwds: [] });

/** The Skills tab — local discovery (what the CLIs already read) plus the
 *  browsable catalog; install/uninstall follow upstream's hardened rules. */
export function SkillsPanel({ onClose }: { onClose: () => void }) {
  const [tab, setTab] = useState<'local' | 'catalog'>('local');
  const [local, setLocal] = useState<LocalSkill[]>([]);
  const [cat, setCat] = useState<CatalogResult | null>(null);
  const [filter, setFilter] = useState('');
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState('');

  const refresh = () => localList().then(setLocal);
  useEffect(() => {
    refresh();
    catalog(false).then(setCat);
  }, []);

  const doInstall = async (s: CatalogSkill) => {
    setBusy(s.name);
    setNote('');
    try {
      const p = await install(s.url, s.name);
      setNote(`Installed ${s.name} → ${p}`);
      refresh();
    } catch (e) {
      setNote(`Install failed: ${String(e)}`);
    }
    setBusy(null);
  };
  const doUninstall = async (s: LocalSkill) => {
    setBusy(s.id);
    try {
      await uninstall(s.path);
      refresh();
    } catch (e) {
      setNote(`Uninstall failed: ${String(e)}`);
    }
    setBusy(null);
  };

  const lc = local.filter(
    (s) =>
      !filter ||
      s.name.toLowerCase().includes(filter.toLowerCase()) ||
      s.description.toLowerCase().includes(filter.toLowerCase()),
  );
  const cc = (cat?.skills || []).filter(
    (s) =>
      !filter ||
      s.name.toLowerCase().includes(filter.toLowerCase()) ||
      s.category.toLowerCase().includes(filter.toLowerCase()),
  );

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div className="modal wide" onClick={(e) => e.stopPropagation()}>
        <h2>Skills</h2>
        <div className="row">
          <button className={tab === 'local' ? '' : 'ghost'} onClick={() => setTab('local')}>
            Installed ({local.length})
          </button>
          <button className={tab === 'catalog' ? '' : 'ghost'} onClick={() => setTab('catalog')}>
            Catalog ({cat?.skills.length ?? '…'})
          </button>
          <input
            placeholder="filter…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            style={{ flex: 1 }}
          />
        </div>
        {note && <div className="agent-sub">{note}</div>}
        {cat?.stale && tab === 'catalog' && (
          <div className="agent-sub">catalog is stale{cat.error ? ` (${cat.error})` : ''}</div>
        )}
        {tab === 'local' &&
          lc.map((s) => (
            <div key={s.id} className="mission-row">
              <div className="grow">
                <div className="agent-name">
                  {s.name} <span className="tag">{s.provider}</span>{' '}
                  <span className="tag">{s.scope}</span>
                </div>
                <div className="agent-sub">{s.description}</div>
              </div>
              <button
                className="ghost"
                disabled={busy === s.id || s.scope === 'bundled'}
                onClick={() => doUninstall(s)}
              >
                {busy === s.id ? '…' : 'Remove'}
              </button>
            </div>
          ))}
        {tab === 'catalog' &&
          cc.slice(0, 80).map((s) => (
            <div key={s.name + s.url} className="mission-row">
              <div className="grow">
                <div className="agent-name">
                  {s.name} <span className="tag">{s.category}</span>{' '}
                  <span className="tag">{s.owner}</span>
                </div>
                <div className="agent-sub">{s.description}</div>
              </div>
              <button disabled={busy === s.name} onClick={() => doInstall(s)}>
                {busy === s.name ? '…' : 'Install'}
              </button>
            </div>
          ))}
        <div className="row end">
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
