/**
 * Multi-window floors in the App shell: the New Floor header button and the
 * Ctrl+Shift+N shortcut exist only while config.multiWindow is on, a floor with
 * live terminals confirms before closing (and stops only its own agents), and a
 * floor window neither re-asks for a hive nor boots a second god.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
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
  useHasTerminalDraft: () => false
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
const NEW_FLOOR = 'New floor (Ctrl+Shift+N)';

// Overridden bridge members are returned as-is (not recorded in `calls`), so
// the two floor commands are plain mocks asserted directly.
const newFloor = vi.fn();
const closeFloor = vi.fn();
beforeEach(() => {
  newFloor.mockReset().mockResolvedValue({ ok: true, label: 'floor-2' });
  closeFloor.mockReset().mockResolvedValue({ ok: true });
});

function bridge(cfg: Record<string, unknown> = {}, overrides: Record<string, unknown> = {}) {
  return installFakeBridge({
    getConfig: async () => ({ ...CONFIG, ...cfg }),
    realtimeHasOpenAiKey: async () => false,
    listPtys: async () => [],
    hiveRegistry: async () => ({ godId: 'god', agents: {} }),
    hiveTasks: async () => [],
    hiveBoard: async () => '',
    drainPendingHires: async () => null,
    newFloor,
    closeFloor,
    ...overrides
  });
}

const calls = (b: ReturnType<typeof bridge>, m: string) => b.calls.filter((c) => c.method === m);

async function ready(container: HTMLElement) {
  await waitFor(() => expect(container.querySelector('[data-testid="office-floor"]')).toBeTruthy());
}

function asFloor(label: string | null): void {
  const w = window as unknown as { __TAURI_INTERNALS__?: unknown };
  if (label === null) delete w.__TAURI_INTERNALS__;
  else w.__TAURI_INTERNALS__ = { metadata: { currentWebview: { label }, currentWindow: { label } } };
}

describe('New Floor entry points', () => {
  beforeEach(() => {
    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    useStore.setState({ agents: [], archivedAgents: [], restorableAgents: [], addAgentOpen: false });
  });
  afterEach(() => asFloor(null));

  it('multiWindow OFF (default): no button, and Ctrl+Shift+N does nothing', async () => {
    bridge();
    const { container } = render(<App />);
    await ready(container);
    expect(screen.queryByLabelText(NEW_FLOOR)).toBeNull();
    fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true });
    expect(newFloor.mock.calls).toHaveLength(0);
  });

  it('multiWindow ON: the header button and the shortcut both open a floor', async () => {
    bridge({ multiWindow: true });
    const { container } = render(<App />);
    await ready(container);
    fireEvent.click(await screen.findByLabelText(NEW_FLOOR));
    await waitFor(() => expect(newFloor.mock.calls).toHaveLength(1));
    fireEvent.keyDown(window, { key: 'N', ctrlKey: true, shiftKey: true });
    await waitFor(() => expect(newFloor.mock.calls).toHaveLength(2));
    // A plain Ctrl+N is not ours.
    fireEvent.keyDown(window, { key: 'n', ctrlKey: true });
    expect(newFloor.mock.calls).toHaveLength(2);
  });

  it('turning the flag on at runtime (config:changed) reveals the button', async () => {
    let live: Record<string, unknown> = {};
    const b = bridge({}, { getConfig: async () => ({ ...CONFIG, ...live }) });
    const { container } = render(<App />);
    await ready(container);
    expect(screen.queryByLabelText(NEW_FLOOR)).toBeNull();
    live = { multiWindow: true }; // what the backend persisted on Save
    act(() => b.emit('onConfigChanged', { multiWindow: true }));
    expect(await screen.findByLabelText(NEW_FLOOR)).toBeTruthy();
  });

  it('a refused newFloor ({ok:false}) is surfaced, not swallowed', async () => {
    newFloor.mockResolvedValue({ ok: false, error: 'floors are off' });
    bridge({ multiWindow: true });
    const { container } = render(<App />);
    await ready(container);
    fireEvent.click(await screen.findByLabelText(NEW_FLOOR));
    expect(await screen.findByText('floors are off')).toBeTruthy();
  });
});

describe('closing a floor with live terminals', () => {
  beforeEach(() => {
    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    useStore.setState({ agents: [], archivedAgents: [], restorableAgents: [], addAgentOpen: false });
  });

  it('asks first; keep-open dismisses, confirm stops this floor and closes it', async () => {
    const b = bridge({ multiWindow: true });
    const { container } = render(<App />);
    await ready(container);

    act(() => b.emit('onFloorCloseRequested', { ptyCount: 2 }));
    expect(await screen.findByText('Close this floor?')).toBeTruthy();
    expect(screen.getByText(/2 terminals are still running on this floor/)).toBeTruthy();

    fireEvent.click(screen.getByText('Keep floor open'));
    await waitFor(() => expect(screen.queryByText('Close this floor?')).toBeNull());
    expect(closeFloor.mock.calls).toHaveLength(0);

    act(() => b.emit('onFloorCloseRequested', { ptyCount: 1 }));
    expect(await screen.findByText(/1 terminal is still running/)).toBeTruthy();
    fireEvent.click(screen.getByText('Stop agents & close floor'));
    await waitFor(() => expect(closeFloor.mock.calls).toHaveLength(1));
  });

  it('a rejected close shows the error and gives the buttons back', async () => {
    closeFloor.mockRejectedValue(new Error('kill failed'));
    const b = bridge();
    const { container } = render(<App />);
    await ready(container);
    act(() => b.emit('onFloorCloseRequested', { ptyCount: 1 }));
    fireEvent.click(await screen.findByText('Stop agents & close floor'));
    expect(await screen.findByText(/Could not close this floor: kill failed/)).toBeTruthy();
    expect((screen.getByText('Keep floor open') as HTMLButtonElement).closest('button')!.disabled).toBe(false);
  });
});

describe('a floor window', () => {
  beforeEach(() => {
    window.localStorage.clear(); // no skipHivePickerOnce: a floor must not need it
    useStore.setState({ agents: [], archivedAgents: [], restorableAgents: [], addAgentOpen: false });
  });
  afterEach(() => asFloor(null));

  it('shows its floor number, skips the hive picker, and never boots a god of its own', async () => {
    asFloor('floor-2');
    const b = bridge({ multiWindow: true });
    const { container } = render(<App />);
    await ready(container); // past the picker straight into the office
    expect(screen.getByTestId('floor-badge').textContent).toBe('Floor 2');
    // Past the god-boot delay (1.2s) in main: a floor must still not spawn.
    await act(async () => { await new Promise((r) => setTimeout(r, 1500)); });
    expect(calls(b, 'spawnPty')).toHaveLength(0);
    expect(useStore.getState().godStatus).toBe('ready');
  });

  it('a floor ignores god-spawned workers that belong to the primary window', async () => {
    asFloor('floor-2');
    const b = bridge();
    const { container } = render(<App />);
    await ready(container);
    act(() => b.emit('onHiveAgentSpawned', { id: 'worker-1', name: 'W', cwd: 'C:/w', provider: 'claude' }));
    await act(async () => { await new Promise((r) => setTimeout(r, 50)); });
    expect(useStore.getState().agents.some((a) => a.id === 'worker-1')).toBe(false);
  });

  it('the primary window DOES card them (control)', async () => {
    window.localStorage.setItem('cth.skipHivePickerOnce', '1');
    const b = bridge();
    const { container } = render(<App />);
    await ready(container);
    expect(screen.queryByTestId('floor-badge')).toBeNull();
    act(() => b.emit('onHiveAgentSpawned', { id: 'worker-1', name: 'W', cwd: 'C:/w', provider: 'claude' }));
    await waitFor(() => expect(useStore.getState().agents.some((a) => a.id === 'worker-1')).toBe(true));
  });
});
