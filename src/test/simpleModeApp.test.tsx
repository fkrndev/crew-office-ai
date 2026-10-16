/**
 * App-level simple mode: the title bar, the empty-floor call to action, and the
 * promise that a legacy config (no uiMode, technical audience) is unchanged.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, waitFor, screen, fireEvent, act } from '@testing-library/react';
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

  it('simple shows a "Stop everything" button in the title bar; advanced (and legacy) does not', async () => {
    bridge({ ...CONFIG, audience: 'non-technical', uiMode: 'simple' });
    const { unmount } = render(<App />);
    const btn = await screen.findByRole('button', { name: 'Stop everything' });
    expect(btn.closest('.cth-titlebar-drag')).toBeTruthy();
    unmount();
    window.localStorage.setItem('cth.skipHivePickerOnce', '1'); // read-once flag: re-arm for the 2nd render
    useStore.setState({ uiMode: 'advanced' } as never);
    bridge(CONFIG);
    render(<App />);
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());
    expect(screen.queryByRole('button', { name: 'Stop everything' })).toBeNull();
  });

  it('the notice banner: raw text in advanced; friendly title + Details in simple', async () => {
    const notice = (detail: Record<string, unknown>) =>
      act(() => { window.dispatchEvent(new CustomEvent('cth:notice', { detail })); });
    bridge(CONFIG);
    const { unmount } = render(<App />);
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());
    notice({ text: 'Could not send: Permission denied (os error 13)', raw: 'Permission denied (os error 13)', context: 'send' });
    expect((await screen.findByRole('alert')).textContent).toContain('Could not send: Permission denied (os error 13)');
    unmount();

    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    useStore.setState({ uiMode: 'advanced' } as never);
    bridge({ ...CONFIG, audience: 'non-technical', uiMode: 'simple' });
    render(<App />);
    await screen.findByTestId('show-advanced-link');
    notice({ text: 'Could not send: Permission denied (os error 13)', raw: 'Permission denied (os error 13)', context: 'send' });
    const friendly = await screen.findByTestId('notice-friendly');
    expect(friendly.textContent).toMatch(/The app isn't allowed to do that/);
    expect(friendly.textContent).not.toMatch(/os error 13/);
    fireEvent.click(screen.getByRole('button', { name: 'Details' }));
    expect(screen.getByRole('region', { name: 'Technical details' }).textContent).toBe('Permission denied (os error 13)');
    // a sentence with no raw error shows its plain variant
    notice({ text: 'tech wording', simpleText: 'plain wording' });
    await screen.findByText('plain wording');
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
