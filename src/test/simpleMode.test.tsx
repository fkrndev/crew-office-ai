/**
 * Simple mode — the UX/layout switch for non-technical users.
 *
 * Pins: how the mode resolves and mirrors into the store, what the Command Center,
 * Add Agent and Settings show in each mode, the plain-language label variants,
 * the "Connect your AI" call to action, locale parity for every key this feature
 * added, and — the promise to existing users — that a legacy config (no uiMode,
 * technical audience) renders exactly the layout it always did.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, act, renderHook } from '@testing-library/react';
import i18n from 'i18next';
import { installFakeBridge } from './fakeBridge';
import { useStore, type Agent } from '@/store/store';
import { resolveUiMode, isSimpleMode } from '@shared/uiMode';
import { useUiMode, persistUiMode, mirrorUiMode } from '@/hooks/useUiMode';
import type { HarnessConfig } from '@/store/config';
import en from '@/i18n/locales/en.json';
import id from '@/i18n/locales/id.json';
import ar from '@/i18n/locales/ar.json';
import zh from '@/i18n/locales/zh-CN.json';

vi.mock('@/components/terminalPool', () => ({
  acquireTerminal: () => ({ term: { cols: 80, rows: 24 }, fit: { fit: () => {} } }),
  releaseTerminal: () => {},
  disposeTerminal: () => {},
  resetTerminal: () => {},
  notifyThemeChangeAll: () => {},
  notifyArabicTerminalChangeAll: () => {},
  focusTerminal: () => {},
  useHasTerminalDraft: () => false,
  getPtyIdForElement: () => undefined
}));
vi.mock('@/components/PtyTerminalView', () => ({ PtyTerminalView: () => <div data-testid="pty-view" /> }));
vi.mock('@/components/MessageQueueComposer', () => ({ MessageQueueComposer: () => <div data-testid="composer" /> }));
vi.mock('@/components/MemoryGraphPanel', () => ({ MemoryGraphPanel: () => null }));
vi.mock('@/realtime/CostHud', () => ({ CostHud: () => <span data-testid="cost-hud" /> }));
vi.mock('@/components/RealtimeMichaelToggle', () => ({ RealtimeMichaelToggle: () => <span data-testid="talk-toggle" /> }));

import { CommandCenterPanel } from '@/components/CommandCenterPanel';
import { AgentCard } from '@/components/AgentCard';
import { AgentStrip } from '@/components/AgentStrip';
import { PixelBadge } from '@/components/PixelBadge';
import { AddAgentModal } from '@/components/AddAgentModal';
import { SettingsModal } from '@/components/SettingsModal';

const BASE_CONFIG = {
  onboardingComplete: true,
  harnessHome: 'C:/hive',
  registeredRepos: [] as string[],
  autoMode: true,
  defaultCommand: 'claude',
  godProvider: 'claude',
  semanticMemory: false,
  embeddingModel: 'minilm'
} as unknown as HarnessConfig;

const god = {
  id: 'god', name: 'Michael', character: 'michael', accent: 'lemon', description: 'god', project: 'hive',
  tmuxTarget: '', cwd: 'C:/hive', status: 'idle', action: 'runs the floor', progress: 0, isGod: true,
  ptyId: 'pty-god'
} as unknown as Agent;

function setMode(mode: 'simple' | 'advanced') {
  act(() => useStore.getState().setUiMode(mode));
}

beforeEach(() => {
  installFakeBridge({
    getConfig: async () => ({ ...BASE_CONFIG }),
    controlSnapshot: async () => null,
    providerKeyHas: async () => false,
    hiveTasks: async () => ({ tasks: [] }),
    hiveLog: async () => [],
    hiveBoard: async () => ''
  });
  useStore.setState({
    uiMode: 'advanced', engineNeedsKey: false, agents: [], hireQueue: { pending: [], reviewed: 0 }
  } as never);
  void i18n.changeLanguage('en');
});

describe('resolveUiMode / useUiMode', () => {
  it('only an explicit uiMode counts; the old audience answer never switches an existing install', () => {
    expect(resolveUiMode({ uiMode: 'simple', audience: 'technical' })).toBe('simple');
    expect(resolveUiMode({ uiMode: 'advanced', audience: 'non-technical' })).toBe('advanced');
    // A legacy install that once answered "non-technical" keeps its layout.
    expect(resolveUiMode({ audience: 'non-technical' })).toBe('advanced');
    expect(resolveUiMode({ audience: 'technical' })).toBe('advanced');
    expect(resolveUiMode({})).toBe('advanced');
    expect(resolveUiMode(null)).toBe('advanced');
    expect(isSimpleMode({ audience: 'non-technical' })).toBe(false);
    expect(isSimpleMode({ uiMode: 'simple' })).toBe(true);
  });

  it('the hook follows the store and starts on advanced', () => {
    const { result } = renderHook(() => useUiMode());
    expect(result.current).toBe('advanced');
    act(() => mirrorUiMode({ uiMode: 'simple' }));
    expect(result.current).toBe('simple');
    act(() => mirrorUiMode({ uiMode: 'advanced', audience: 'non-technical' }));
    expect(result.current).toBe('advanced');
  });

  it('persistUiMode: simple also sets the audience; advanced never touches it; a failed write reverts', async () => {
    const updateConfig = vi.fn(async (p: unknown) => p);
    installFakeBridge({ updateConfig });
    await persistUiMode('simple');
    expect(updateConfig).toHaveBeenLastCalledWith({ uiMode: 'simple', audience: 'non-technical' });
    expect(useStore.getState().uiMode).toBe('simple');
    await persistUiMode('advanced');
    expect(updateConfig).toHaveBeenLastCalledWith({ uiMode: 'advanced' });
    expect(useStore.getState().uiMode).toBe('advanced');

    installFakeBridge({ updateConfig: async () => { throw new Error('disk full'); } });
    expect(await persistUiMode('simple')).toBe(false);
    expect(useStore.getState().uiMode).toBe('advanced');
  });
});

describe('Command Center tab gating', () => {
  const tabNames = () => screen.getAllByRole('button').map((b) => (b.textContent ?? '').trim().toLowerCase());

  it('advanced (and legacy) shows every tab, the IDE and the delivery switch', () => {
    render(<CommandCenterPanel agent={god} />);
    const names = tabNames();
    for (const n of ['terminal', 'monitor', 'tasks', 'ask me', 'triggers', 'memory', 'graph', 'activity', 'skills', 'workers']) {
      expect(names.some((x) => x.endsWith(n))).toBe(true);
    }
    expect(names.some((x) => x.endsWith('ide'))).toBe(true);
    expect(screen.getByText('COMMAND CENTER')).toBeTruthy();
  });

  it('simple shows only talk / tasks / questions / activity, and no IDE or delivery switch', () => {
    setMode('simple');
    render(<CommandCenterPanel agent={god} />);
    const names = tabNames();
    expect(names.some((x) => x.endsWith('talk'))).toBe(true);
    expect(names.some((x) => x.endsWith('tasks'))).toBe(true);
    expect(names.some((x) => x.endsWith('questions'))).toBe(true);
    expect(names.some((x) => x.endsWith('activity'))).toBe(true);
    for (const n of ['monitor', 'triggers', 'memory', 'graph', 'skills', 'workers', 'ide', 'auto', 'paused']) {
      expect(names.some((x) => x.endsWith(n))).toBe(false);
    }
    expect(screen.getByText('YOUR TEAM')).toBeTruthy();
    expect(screen.getByText('Michael leads your team')).toBeTruthy();
    // the conversation (terminal + composer) is the first tab and stays
    expect(screen.getByTestId('composer')).toBeTruthy();
  });

  it('switching to simple while on a hidden tab falls back to talk', async () => {
    render(<CommandCenterPanel agent={god} />);
    fireEvent.click(screen.getByRole('button', { name: /graph/i }));
    setMode('simple');
    await waitFor(() => expect(screen.getByTestId('composer')).toBeTruthy());
  });
});

describe('plain-language label variants', () => {
  it('badges and cards say plain things in simple mode only', () => {
    const { unmount } = render(<PixelBadge status="idle" />);
    expect(screen.getByText('idle')).toBeTruthy();
    unmount();
    setMode('simple');
    render(<PixelBadge status="blocked" />);
    expect(screen.getByText('needs you')).toBeTruthy();
  });

  it('the agent card swaps the folder / tool line for a friendly status and drops the gauge', () => {
    const props = {
      name: 'Pam', character: 'pam' as const, accent: 'mint' as const, status: 'working' as const,
      project: 'my-repo', action: 'using Bash', progress: 3
    };
    const { container, unmount } = render(<AgentCard {...props} />);
    expect(screen.getByText('using Bash')).toBeTruthy();
    expect(container.querySelector('[title*="context" i]')).toBeTruthy();
    unmount();
    setMode('simple');
    const r = render(<AgentCard {...props} />);
    expect(screen.getByText('Working on it…')).toBeTruthy();
    expect(screen.queryByText('using Bash')).toBeNull();
    expect(screen.queryByText('my-repo')).toBeNull();
    expect(r.container.querySelector('[title*="context" i]')).toBeNull();
  });

  it('the orchestrator card shows one "Connect your AI" button when the engine needs a key', () => {
    setMode('simple');
    const onConnect = vi.fn();
    render(<AgentCard name="Michael" character="michael" accent="lemon" status="idle" project="hive"
      isGod needsConnection onConnect={onConnect} />);
    fireEvent.click(screen.getByText('Connect your AI'));
    expect(onConnect).toHaveBeenCalled();
    expect(screen.queryByTestId('cost-hud')).toBeNull();
    expect(screen.queryByTestId('talk-toggle')).toBeNull();
  });

  it('the strip shows the CTA on an orchestrator that exited (nothing usable behind it) — simple only', () => {
    const dead = { ...god, status: 'ghost' } as Agent;
    useStore.setState({ agents: [dead], restorableAgents: [] } as never);
    const { unmount } = render(<AgentStrip config={BASE_CONFIG} />);
    expect(screen.queryByText('Connect your AI')).toBeNull();
    unmount();
    setMode('simple');
    render(<AgentStrip config={BASE_CONFIG} />);
    expect(screen.getByText('Connect your AI')).toBeTruthy();
  });

  it('the Connect-your-AI call opens Settings on the AI connection tab', () => {
    const seen: unknown[] = [];
    const on = (e: Event) => seen.push((e as CustomEvent).detail);
    window.addEventListener('cth:open-settings', on);
    void import('@/hooks/useUiMode').then((m) => m.openAiConnection());
    return waitFor(() => expect(seen).toEqual([{ section: 'AI connection' }]))
      .finally(() => window.removeEventListener('cth:open-settings', on));
  });
});

describe('Add Agent', () => {
  it('simple: one short form with a suggested name, a goal box and one primary button', () => {
    setMode('simple');
    render(<AddAgentModal onClose={() => {}} config={BASE_CONFIG} />);
    expect((screen.getByPlaceholderText('Ada') as HTMLInputElement).value).toBe('Jim');
    expect(screen.getByText('What should this teammate do?')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Add teammate' })).toBeTruthy();
    for (const r of ['Researcher', 'Writer', 'Problem solver', 'Organizer']) {
      expect(screen.getByRole('button', { name: r })).toBeTruthy();
    }
    // nothing technical until "More options"
    expect(screen.queryByText('Provider')).toBeNull();
    expect(screen.queryByText(/^Command/)).toBeNull();
    expect(screen.queryByText(/own copy of the files/)).toBeNull();
    expect(screen.queryByText('Workspace')).toBeNull();
    expect(screen.queryByText('Agent templates')).toBeNull();
  });

  it('simple: the suggested name skips teammates already on the floor', () => {
    setMode('simple');
    useStore.setState({ agents: [{ ...god, id: 'jim', name: 'Jim', isGod: false } as Agent] } as never);
    render(<AddAgentModal onClose={() => {}} config={BASE_CONFIG} />);
    expect((screen.getByPlaceholderText('Ada') as HTMLInputElement).value).toBe('Pam');
  });

  it('simple: the advanced fields appear under "More options"; engine chips lead with Hermes cost hints', () => {
    setMode('simple');
    render(<AddAgentModal onClose={() => {}} config={BASE_CONFIG} />);
    const more = screen.getByTestId('add-agent-more');
    expect(more.getAttribute('aria-expanded')).toBe('false');
    fireEvent.click(more);
    expect(more.getAttribute('aria-expanded')).toBe('true');
    expect(screen.getByText('Which AI')).toBeTruthy();
    expect(screen.getByText(/^Start-up command/)).toBeTruthy();
    expect(screen.getByText(/own copy of the files/)).toBeTruthy();
    expect(screen.queryByText(/Resume session/i)).toBeNull();
    expect(screen.getByText('Saved setups')).toBeTruthy();
    expect(screen.getByText('Recommended · low cost')).toBeTruthy();
    expect(screen.getByText('Higher cost')).toBeTruthy();
  });

  it('simple: a role idea fills the goal; an empty goal still adds (defaults apply)', () => {
    setMode('simple');
    render(<AddAgentModal onClose={() => {}} config={BASE_CONFIG} />);
    fireEvent.click(screen.getByRole('button', { name: 'Researcher' }));
    const goal = screen.getByPlaceholderText(/keep my notes organized/i) as HTMLTextAreaElement;
    expect(goal.value).toMatch(/^Research the questions/);
  });

  it('advanced (and legacy) keeps today\'s sectioned form, with cost hints on the engine chips', () => {
    render(<AddAgentModal onClose={() => {}} config={BASE_CONFIG} />);
    expect(screen.getByText('Identity')).toBeTruthy();
    expect(screen.getByText('Workspace')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'create' })).toBeTruthy();
    expect(screen.queryByTestId('add-agent-more')).toBeNull();
    fireEvent.click(screen.getByText('Engine'));
    expect(screen.getByText('Recommended · low cost')).toBeTruthy();
    expect(screen.getByTestId('engine-cost-hint').textContent).toMatch(/Claude uses your Claude subscription/);
  });
});

describe('Settings', () => {
  const navLabels = () =>
    ['General', 'Prerequisites', 'Agents & Models', 'Autonomy & Budgets', 'Connections', 'Voice', 'Memory & Knowledge',
      'AI connection', 'Updates', 'About']
      .filter((l) => screen.queryAllByRole('button').some((b) => (b.textContent ?? '').trim().toLowerCase() === l.toLowerCase()));

  it('advanced (and legacy) keeps all seven tabs and the danger zone', () => {
    render(<SettingsModal config={BASE_CONFIG} onClose={() => {}} />);
    expect(navLabels()).toEqual(['General', 'Prerequisites', 'Agents & Models', 'Autonomy & Budgets', 'Connections', 'Voice', 'Memory & Knowledge']);
    expect(screen.getByTestId('advanced-switch')).toBeTruthy();
    expect(screen.queryByTestId('simple-settings-note')).toBeNull();
  });

  it('simple shows General / AI connection / Updates / About, a hidden-settings note, no danger zone', () => {
    render(<SettingsModal config={{ ...BASE_CONFIG, uiMode: 'simple' } as HarnessConfig} onClose={() => {}} />);
    expect(navLabels()).toEqual(['General', 'AI connection', 'Updates', 'About']);
    expect(screen.getByTestId('simple-settings-note')).toBeTruthy();
    expect(screen.queryByText(/reset & start over/i)).toBeNull();
    expect(screen.getByTestId('theme-switch')).toBeTruthy();
    expect(screen.getByTestId('explain-switch')).toBeTruthy();
    expect(screen.getByTestId('advanced-switch')).toBeTruthy();
    // none of the technical sections' controls
    expect(screen.queryByText(/keep awake/i)).toBeNull();
    expect(screen.queryByText(/home folder/i)).toBeNull();
  });

  it('a legacy install that answered non-technical (no uiMode) keeps the full Settings', () => {
    render(<SettingsModal config={{ ...BASE_CONFIG, audience: 'non-technical' } as HarnessConfig} onClose={() => {}} />);
    expect(navLabels()).not.toEqual(['General', 'AI connection', 'Updates', 'About']);
  });

  it('a deep link to the AI connection tab lands there in simple, and on Agents & Models in advanced', () => {
    const { unmount } = render(
      <SettingsModal config={{ ...BASE_CONFIG, uiMode: 'simple' } as HarnessConfig} onClose={() => {}} initialSection="AI connection" />
    );
    expect(screen.getByTestId('simple-ai-connection')).toBeTruthy();
    // Hermes leads; Claude is the other primary chip; both carry a cost tag
    const chips = screen.getAllByRole('button', { pressed: false }).concat(screen.getAllByRole('button', { pressed: true }));
    const labels = chips.map((c) => c.textContent ?? '');
    expect(labels.findIndex((l) => l.includes('Hermes'))).toBeLessThan(labels.findIndex((l) => l.includes('Claude')));
    expect(screen.getByText('Recommended · low cost')).toBeTruthy();
    unmount();
    render(<SettingsModal config={BASE_CONFIG} onClose={() => {}} initialSection="AI connection" />);
    expect(screen.queryByTestId('simple-ai-connection')).toBeNull();
    expect(screen.getAllByText(/api keys/i).length).toBeGreaterThan(0);
  });

  it('turning the advanced switch off stages uiMode:simple AND audience:non-technical through Save', async () => {
    const updateConfig = vi.fn(async (p: unknown) => ({ ...BASE_CONFIG, ...(p as object) }));
    installFakeBridge({ getConfig: async () => ({ ...BASE_CONFIG }), updateConfig, providerKeyHas: async () => false });
    render(<SettingsModal config={BASE_CONFIG} onClose={() => {}} />);
    fireEvent.click(screen.getByTestId('advanced-switch').querySelector('button')!);
    // the form re-lays out at once …
    await waitFor(() => expect(screen.getByTestId('simple-settings-note')).toBeTruthy());
    expect(updateConfig).not.toHaveBeenCalled();
    // … and the app follows on Save
    fireEvent.click(screen.getByRole('button', { name: 'save' }));
    await waitFor(() => expect(updateConfig).toHaveBeenCalled());
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch.uiMode).toBe('simple');
    expect(patch.audience).toBe('non-technical');
    expect(useStore.getState().uiMode).toBe('simple');
  });

  it('turning advanced ON does not force the audience back', async () => {
    const updateConfig = vi.fn(async (p: unknown) => ({ ...BASE_CONFIG, ...(p as object) }));
    installFakeBridge({ getConfig: async () => ({ ...BASE_CONFIG }), updateConfig, providerKeyHas: async () => false });
    render(<SettingsModal config={{ ...BASE_CONFIG, uiMode: 'simple', audience: 'non-technical' } as HarnessConfig} onClose={() => {}} />);
    fireEvent.click(screen.getByTestId('advanced-switch').querySelector('button')!);
    await waitFor(() => expect(screen.queryByTestId('simple-settings-note')).toBeNull());
    fireEvent.click(screen.getByRole('button', { name: 'save' }));
    await waitFor(() => expect(updateConfig).toHaveBeenCalled());
    const patch = updateConfig.mock.calls[0][0] as Record<string, unknown>;
    expect(patch.uiMode).toBe('advanced');
    expect('audience' in patch).toBe(false);
  });

  it('the one-click link in the simple footer writes immediately', async () => {
    const updateConfig = vi.fn(async (p: unknown) => p);
    installFakeBridge({ getConfig: async () => ({ ...BASE_CONFIG }), updateConfig, providerKeyHas: async () => false });
    render(<SettingsModal config={{ ...BASE_CONFIG, uiMode: 'simple' } as HarnessConfig} onClose={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Need more? Show advanced options' }));
    await waitFor(() => expect(updateConfig).toHaveBeenCalledWith({ uiMode: 'advanced' }));
    await waitFor(() => expect(screen.queryByTestId('simple-settings-note')).toBeNull());
  });
});

describe('locale parity for the simple-mode keys', () => {
  const flat = (o: unknown, prefix = ''): string[] =>
    Object.entries(o as Record<string, unknown>).flatMap(([k, v]) =>
      v && typeof v === 'object' ? flat(v, `${prefix}${k}.`) : [`${prefix}${k}`]);
  const mine = (keys: string[]) =>
    keys.filter((k) => k.startsWith('simpleMode.') || k.startsWith('engineCost.') || /_simple$/.test(k)
      || k.startsWith('settings.nav.')).sort();

  it('every locale has exactly the same simpleMode / engineCost / _simple / settings.nav keys', () => {
    const base = mine(flat(en));
    expect(base.length).toBeGreaterThan(80);
    expect(mine(flat(id))).toEqual(base);
    expect(mine(flat(ar))).toEqual(base);
    expect(mine(flat(zh))).toEqual(base);
  });

  it('keeps interpolation placeholders identical across locales', () => {
    const get = (o: unknown, path: string) => path.split('.').reduce((a, p) => (a as Record<string, unknown>)?.[p], o) as string;
    const ph = (s: string) => (s.match(/{{\s*\w+\s*}}/g) ?? []).sort().join(',');
    for (const k of mine(flat(en))) {
      for (const loc of [id, ar, zh]) {
        expect(ph(get(loc, k)), k).toBe(ph(get(en, k)));
      }
    }
  });

  it('every _simple variant has a base key to fall back on', () => {
    const all = new Set(flat(en));
    for (const k of flat(en).filter((x) => x.endsWith('_simple'))) {
      expect(all.has(k.slice(0, -'_simple'.length)), k).toBe(true);
    }
  });
});
