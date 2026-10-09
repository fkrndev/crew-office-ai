/**
 * ShardX card busy-state regression — every button shares one `busy` flag, so
 * a hung bootstrap used to leave the whole card dead-disabled forever with no
 * visible reason. The contract now:
 *
 * - while an action runs: buttons disabled AND a "working…" note is visible;
 * - when it resolves (ok or not) or throws: buttons re-enable;
 * - backend timeouts bound the hang server-side (cargo side).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { McpDefaultsSettings } from '@/components/McpDefaultsSettings';
import type { HarnessConfig } from '@/store/config';

let cth: FakeBridge;
const config = { mcpDefaults: {} } as unknown as HarnessConfig;

beforeEach(() => {
  cth = installFakeBridge();
});

afterEach(() => {
  vi.restoreAllMocks();
});

const autoSetupBtn = () =>
  screen.getByText('Auto setup').closest('button')!;

describe('ShardX card busy lifecycle', () => {
  it('in-flight action disables buttons AND shows the working note', async () => {
    let release!: (v: unknown) => void;
    cth.shardxBootstrap = vi.fn(
      () => new Promise((r) => (release = r)) // hangs like the old no-timeout path
    );
    render(<McpDefaultsSettings config={config} />);
    const btn = autoSetupBtn();
    expect(btn.disabled).toBe(false);

    fireEvent.click(btn);

    await waitFor(() => expect(btn.disabled).toBe(true));
    // THE bug: previously the card gave no sign anything was running.
    await waitFor(() => expect(screen.getByText(/working/i)).toBeTruthy());
    expect(cth.shardxBootstrap).toHaveBeenCalledTimes(1);

    release({ ok: true });
    await waitFor(() => expect(btn.disabled).toBe(false));
  });

  it('a rejected action re-enables the card instead of pinning it', async () => {
    cth.shardxBootstrap = vi.fn(async () => {
      throw new Error('dead');
    });
    render(<McpDefaultsSettings config={config} />);
    const btn = autoSetupBtn();
    fireEvent.click(btn);
    await waitFor(() => expect(btn.disabled).toBe(true));
    await waitFor(() => expect(btn.disabled).toBe(false));
    await waitFor(() => expect(screen.getByText(/failed/i)).toBeTruthy());
  });

  it('a failed-result action re-enables and reports the failing step', async () => {
    cth.shardxBootstrap = vi.fn(async () => ({
      ok: false,
      steps: { launcher: { ok: false }, token: { ok: false }, bridge: { ok: true } }
    }));
    render(<McpDefaultsSettings config={config} />);
    const btn = autoSetupBtn();
    fireEvent.click(btn);
    await waitFor(() => expect(btn.disabled).toBe(false));
    // The failing gate names ride the failure note — the card answers WHICH
    // step went red, not just "failed".
    await waitFor(() =>
      expect(screen.getByText(/failed.*launcher/i)).toBeTruthy()
    );
  });
});
