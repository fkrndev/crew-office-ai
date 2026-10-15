/**
 * App-level simple mode: the title bar, the empty-floor call to action, and the
 * promise that a legacy config (no uiMode, technical audience) is unchanged.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, waitFor, screen, fireEvent } from '@testing-library/react';
import { installFakeBridge } from './fakeBridge';
import { useStore } from '@/store/store';

vi.mock('@/scene/office/OfficeFloor', () => ({
  OfficeFloor: () => <div data-testid="office-floor" />
}));
vi.mock('@/components/terminalPool', () => ({
  acquireTerminal: () => {},
  releaseTerminal: () => {},
  notifyThemeChangeAll: () => {},
  focusTerminal: () => {},
  suspendTerminal: () => {},
  isTerminalLive: () => false,
  detachTerminal: () => {},
  resizeTerminal: () => {},
  writeToTerminal: () => {},
  setLinkProvider: () => {},
  getPtyIdForElement: () => undefined,
  resetTerminal: () => {}
}));
vi.mock('@/components/FullscreenTerminal', () => ({ FullscreenTerminal: () => null }));
vi.mock('@/ide/IdePanel', () => ({ IdePanel: () => null }));
vi.mock('@/components/AddAgentModal', () => ({ AddAgentModal: () => null }));
vi.mock('@/components/SettingsModal', () => ({ SettingsModal: () => null }));
vi.mock('@/components/TaskDetailOverlay', () => ({ TaskDetailOverlay: () => null }));
vi.mock('@/components/MemoryPanel', () => ({ MemoryPanel: () => null }));
vi.mock('@/components/AgentDetailPanel', () => ({ AgentDetailPanel: () => null }));
vi.mock('@/freeflow/holdOption', () => ({ useHoldOptionToTalk: () => {} }));
vi.mock('@/hooks/useHive', () => ({ useHive: () => {} }));

import { App } from '@/App';

const CONFIG = {
  onboardingComplete: true,
  audience: 'technical',
  harnessHome: 'C:\\Users\\t\\HarnessAgents',
  registeredRepos: [],
  autoMode: true,
  godProvider: 'claude',
  telemetryEnabled: true,
  missions: [],
  notifications: false,
  webhooks: [],
  secretEnvKeys: []
};

function bridge(cfg: Record<string, unknown>, overrides: Record<string, unknown> = {}) {
  return installFakeBridge({
    getConfig: async () => ({ ...cfg }),
    realtimeHasOpenAiKey: async () => false,
    listPtys: async () => [],
    hiveRegistry: async () => ({ godId: 'god', agents: {} }),
    hiveTasks: async () => [],
    hiveBoard: async () => '',
    drainPendingHires: async () => null,
    ...overrides
  });
}

describe('App in simple vs advanced mode', () => {
  beforeEach(() => {
    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    useStore.setState({
      agents: [], archivedAgents: [], restorableAgents: [], addAgentOpen: false,
      uiMode: 'advanced', engineNeedsKey: false, godStatus: 'failed'
    } as never);
  });

  it('a legacy config renders the unchanged title bar and empty floor', async () => {
    bridge(CONFIG);
    render(<App />);
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());
    expect(screen.queryByTestId('show-advanced-link')).toBeNull();
    expect(screen.getByText('EMPTY FLOOR')).toBeTruthy();
    expect(screen.queryByText('Connect your AI')).toBeNull();
    expect(screen.getByLabelText('Toggle focus mode')).toBeTruthy();
    expect(useStore.getState().uiMode).toBe('advanced');
  });

  it('a non-technical config is simple: no auto-mode chip, a way back, and a Connect-your-AI call to action', async () => {
    bridge({ ...CONFIG, audience: 'non-technical', uiMode: 'simple' });
    render(<App />);
    await waitFor(() => expect(screen.getByTestId('show-advanced-link')).toBeTruthy());
    expect(screen.queryByText('auto mode on')).toBeNull();
    expect(screen.queryByLabelText('Toggle focus mode')).toBeNull();
    expect(screen.getByText('WELCOME')).toBeTruthy();
    expect(screen.getAllByText('add teammate').length).toBeGreaterThan(0);

    const seen: unknown[] = [];
    window.addEventListener('cth:open-settings', (e) => seen.push((e as CustomEvent).detail));
    fireEvent.click(screen.getByText('Connect your AI'));
    expect(seen).toContainEqual({ section: 'AI connection' });
  });

  it('"Need more? Show advanced options" writes uiMode and flips the layout', async () => {
    const updateConfig = vi.fn(async (p: unknown) => p);
    bridge({ ...CONFIG, uiMode: 'simple' }, { updateConfig });
    render(<App />);
    fireEvent.click(await screen.findByTestId('show-advanced-link'));
    await waitFor(() => expect(updateConfig).toHaveBeenCalledWith({ uiMode: 'advanced' }));
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());
  });
});
