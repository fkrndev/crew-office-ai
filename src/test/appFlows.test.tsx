/**
 * App-level flows that misfired against the real backend's return shapes:
 *  - config:changed carries PARTIAL payloads ({} / {autoMode}) — replacing the
 *    whole config with one dropped onboardingComplete and re-showed the wizard.
 *  - closing_time_start resolves a bare worker count, not {ok} — treating that as
 *    failure showed an error for a protocol that had started.
 *  - the quit dialog's "kill & quit" must reach the new force-quit command.
 *  - deep-link hires arrive as a SOURCE string (hire-pending / drain), which has
 *    to be fetched + validated before it reaches the review queue.
 *  - a rejected first config read used to be a permanent blank screen.
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
  getPtyIdForElement: () => undefined
}));
vi.mock('@/components/FullscreenTerminal', () => ({ FullscreenTerminal: () => null }));
vi.mock('@/ide/IdePanel', () => ({ IdePanel: () => null }));
vi.mock('@/components/AddAgentModal', () => ({ AddAgentModal: () => null }));
vi.mock('@/components/SettingsModal', () => ({ SettingsModal: () => null }));
vi.mock('@/components/TaskDetailOverlay', () => ({ TaskDetailOverlay: () => null }));
vi.mock('@/components/MemoryPanel', () => ({ MemoryPanel: () => null }));
vi.mock('@/components/AgentDetailPanel', () => ({ AgentDetailPanel: () => null }));
vi.mock('@/freeflow/holdOption', () => ({ useHoldOptionToTalk: () => {} }));

import { App } from '@/App';

const CONFIG = {
  onboardingComplete: true,
  audience: 'technical',
  harnessHome: 'C:\\Users\\t\\HarnessAgents',
  registeredRepos: [],
  autoMode: true,
  godProvider: 'claude',
  godModel: 'claude-opus-4-8',
  telemetryEnabled: true,
  missions: [],
  notifications: false,
  webhooks: [],
  secretEnvKeys: []
};

function bridge(overrides: Record<string, unknown> = {}) {
  return installFakeBridge({
    getConfig: async () => ({ ...CONFIG }),
    realtimeHasOpenAiKey: async () => false,
    listPtys: async () => [],
    hiveRegistry: async () => ({ godId: 'god', agents: {} }),
    hiveTasks: async () => [],
    hiveBoard: async () => '',
    drainPendingHires: async () => null,
    ...overrides
  });
}

describe('App backend-shape flows', () => {
  beforeEach(() => {
    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    useStore.setState({ agents: [], archivedAgents: [], restorableAgents: [], addAgentOpen: false });
    useStore.getState().clearPendingHires();
  });

  it('a PARTIAL config:changed payload merges — it never bounces the user back to onboarding', async () => {
    let cfg: Record<string, unknown> = { ...CONFIG };
    const b = bridge({ getConfig: async () => ({ ...cfg }) });
    const { container } = render(<App />);
    await waitFor(() => expect(container.querySelector('[data-testid="office-floor"]')).toBeTruthy());
    expect(screen.getByText('auto mode on')).toBeTruthy();

    // set_auto_mode emits just {autoMode}; the voice actions emit {}.
    cfg = { ...cfg, autoMode: false };
    act(() => b.emit('onConfigChanged', { autoMode: false }));
    await waitFor(() => expect(screen.getByText('auto mode off')).toBeTruthy());
    expect(container.querySelector('[data-testid="office-floor"]')).toBeTruthy();

    act(() => b.emit('onConfigChanged', {}));
    await new Promise((r) => setTimeout(r, 50));
    expect(container.querySelector('[data-testid="office-floor"]')).toBeTruthy();
  });

  it('a rejected first config read shows an error with Retry, not a blank screen', async () => {
    let calls = 0;
    bridge({
      getConfig: async () => {
        calls += 1;
        if (calls === 1) throw new Error('config.json unreadable');
        return { ...CONFIG };
      }
    });
    const { container } = render(<App />);
    await waitFor(() => expect(screen.getByText(/config\.json unreadable/)).toBeTruthy());
    fireEvent.click(screen.getByText('retry'));
    await waitFor(() => expect(container.querySelector('[data-testid="office-floor"]')).toBeTruthy());
  });

  it('closing time: a resolved worker count is success; a rejection shows the real reason', async () => {
    const startClosingTime = vi.fn(async () => 3);
    const b = bridge({ startClosingTime });
    render(<App />);
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());
    act(() => b.emit('onCloseRequested', { ptyCount: 2 }));
    await waitFor(() => expect(screen.getByText(/2 AGENTS STILL RUNNING/)).toBeTruthy());

    fireEvent.click(screen.getByRole('button', { name: /closing time/i }));
    await waitFor(() => expect(startClosingTime).toHaveBeenCalledTimes(1));
    // Protocol armed → no failure box.
    expect(screen.queryByText('Closing time could not start.')).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();

    startClosingTime.mockRejectedValueOnce('no god agent is running');
    fireEvent.click(screen.getByRole('button', { name: /closing time/i }));
    await waitFor(() => expect(screen.getByText('no god agent is running')).toBeTruthy());
  });

  it('"kill all & quit" calls the force-quit command (not confirmClose)', async () => {
    const forceQuit = vi.fn(async () => true);
    const confirmClose = vi.fn(async () => 1);
    const b = bridge({ forceQuit, confirmClose });
    render(<App />);
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());
    act(() => b.emit('onCloseRequested', { ptyCount: 2 }));
    await waitFor(() => screen.getByText('kill all & quit'));
    fireEvent.click(screen.getByText('kill all & quit'));
    await waitFor(() => expect(forceQuit).toHaveBeenCalledTimes(1));
    expect(confirmClose).not.toHaveBeenCalled();
  });

  it('hire-pending: the source URL is fetched, validated and queued for review', async () => {
    const manifest = { name: 'Dana', description: 'a closer' };
    const hireFetch = vi.fn(async () => ({ ok: true, manifest }));
    const drainPendingHires = vi.fn(async () => null);
    const b = bridge({ hireFetch, drainPendingHires });
    render(<App />);
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());

    act(() => b.emit('onHirePending', 'https://h.example/dana.json'));
    await waitFor(() => expect(hireFetch).toHaveBeenCalledWith('https://h.example/dana.json'));
    await waitFor(() => expect(useStore.getState().addAgentOpen).toBe(true));
    expect(useStore.getState().hireQueue.pending).toEqual([manifest]);
    // The parked copy is drained so it is not imported again next launch.
    expect(drainPendingHires).toHaveBeenCalledTimes(2); // mount + the event
  });

  it('cold start: a parked deep-link source (a STRING, not an array) is imported', async () => {
    const manifest = { name: 'Cold' };
    const hireFetch = vi.fn(async () => ({ ok: true, manifest }));
    bridge({ hireFetch, drainPendingHires: async () => 'https://h.example/cold.json' });
    render(<App />);
    await waitFor(() => expect(hireFetch).toHaveBeenCalledWith('https://h.example/cold.json'));
    await waitFor(() => expect(useStore.getState().hireQueue.pending).toEqual([manifest]));
  });

  it('a failed deep-link import is a visible banner, not a console line', async () => {
    const hireFetch = vi.fn(async () => { throw 'manifest host resolves to a blocked (private/reserved) address'; });
    const b = bridge({ hireFetch });
    render(<App />);
    await waitFor(() => expect(screen.getByText('auto mode on')).toBeTruthy());
    act(() => b.emit('onHirePending', 'https://10.0.0.1/m.json'));
    await waitFor(() => expect(screen.getByText(/Could not import the agent: manifest host resolves/)).toBeTruthy());
    expect(useStore.getState().addAgentOpen).toBe(false);
  });
});
