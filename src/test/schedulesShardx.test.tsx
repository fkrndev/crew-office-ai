/**
 * Scheduler → ShardX automation wiring. A mission with kind 'shardx' replays
 * a launcher automation project instead of mailing an agent — body is the
 * project id, and the editor swaps the prompt textarea for a project picker.
 *
 * Contracts covered:
 * - adding a schedule with the "ShardX automation" target persists
 *   { to: 'shardx', kind: 'shardx', body: <project id> };
 * - a stored shardx mission shows the project NAME (not the raw uuid) and
 *   "ShardX automation" as its target;
 * - retargeting a shardx mission to an agent clears kind — otherwise the
 *   scheduler would keep replaying the project forever;
 * - the prompt field never renders for shardx missions.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { SchedulesSection } from '@/components/triggers/SchedulesSection';

let cth: FakeBridge;

const projects = [
  { id: 'proj-a', name: 'reply-buyers' },
  { id: 'proj-b', name: 'price-watch' }
];

const shardxMission = {
  id: 'm_sx',
  label: 'Daily buyer replies',
  intervalMs: 86_400_000,
  to: 'shardx',
  body: 'proj-a',
  enabled: true,
  kind: 'shardx' as const
};

const savedMissions = () =>
  (cth.saveMissions as ReturnType<typeof vi.fn>).mock.calls[0][0] as Array<Record<string, unknown>>;

beforeEach(() => {
  cth = installFakeBridge({
    listMissions: async () => [],
    saveMissions: vi.fn(async () => undefined),
    shardxAutomations: vi.fn(async () => ({ ok: true, projects }))
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('schedule → shardx automation', () => {
  it('adds a mission that replays a launcher project on the cadence', async () => {
    render(<SchedulesSection />);
    fireEvent.click(screen.getByText('add a schedule'));

    const label = screen.getByPlaceholderText(/what this run is for/i);
    fireEvent.change(label, { target: { value: 'morning replies' } });

    // Target "ShardX automation" swaps the prompt textarea for a project pick.
    const goesTo = screen.getAllByRole('combobox')[0];
    fireEvent.change(goesTo, { target: { value: 'shardx' } });
    await waitFor(() =>
      expect(screen.getByText('AUTOMATION PROJECT')).toBeTruthy()
    );
    expect(screen.queryByPlaceholderText(/sent word for word/i)).toBeNull();

    const projectPick = screen.getByText('AUTOMATION PROJECT')
      .closest('div')!.parentElement!.querySelector('select')!;
    fireEvent.change(projectPick, { target: { value: 'proj-b' } });

    fireEvent.click(screen.getByText('add'));
    await waitFor(() => expect(cth.saveMissions).toHaveBeenCalled());
    const m = savedMissions()[0];
    expect(m.kind).toBe('shardx');
    expect(m.to).toBe('shardx');
    expect(m.body).toBe('proj-b');
    expect(m.label).toBe('morning replies');
    expect(m.enabled).toBe(true);
  });

  it('a shardx mission row shows the project name, not the uuid', async () => {
    cth.listMissions = vi.fn(async () => [shardxMission]);
    render(<SchedulesSection />);
    await waitFor(() => expect(screen.getByText('Daily buyer replies')).toBeTruthy());
    // Preview line resolves the stored id to the project name.
    expect(screen.getByText(/reply-buyers/)).toBeTruthy();
    expect(screen.queryByText(/proj-a/)).toBeNull();
    // And the target resolves to the automation label.
    expect(screen.getByText(/ShardX automation/)).toBeTruthy();
  });

  it('retargeting a shardx mission to an agent clears the kind', async () => {
    cth.listMissions = vi.fn(async () => [shardxMission]);
    render(<SchedulesSection />);
    await waitFor(() => expect(screen.getByText('Daily buyer replies')).toBeTruthy());
    // Expand the row, repoint "goes to" at broadcast, save.
    fireEvent.click(screen.getByText('Daily buyer replies'));
    await waitFor(() => expect(screen.getByText('GOES TO')).toBeTruthy());
    const goesTo = screen.getByText('GOES TO').closest('div')!.parentElement!
      .querySelector('select')!;
    fireEvent.change(goesTo, { target: { value: 'broadcast' } });
    fireEvent.click(screen.getByText('save'));
    await waitFor(() => expect(cth.saveMissions).toHaveBeenCalled());
    const m = savedMissions()[0];
    expect(m.kind).toBeUndefined();
    expect(m.to).toBe('broadcast');
  });

  it('missions saved from real upstream keep working when shardx is absent', async () => {
    // Regression guard: a plain dispatch mission still mail-targets an agent.
    cth.listMissions = vi.fn(async () => [{
      id: 'm_plain', label: 'standup', intervalMs: 3_600_000,
      to: 'god', body: 'summarize yesterday', enabled: true
    }]);
    render(<SchedulesSection />);
    await waitFor(() => expect(screen.getByText('standup')).toBeTruthy());
    expect(screen.getByText(/summarize yesterday/)).toBeTruthy();
  });
});
