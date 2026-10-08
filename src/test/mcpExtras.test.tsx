/**
 * Per-agent MCP opt-in — the "extra tools" checkboxes in AddAgentModal. A
 * checked write/secret catalog id rides the spawn payload as mcpEnable so the
 * backend merge can arm it for THIS agent only.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { AddAgentModal } from '@/components/AddAgentModal';
import type { HarnessConfig } from '@/store/config';
import { useStore } from '@/store/store';

const baseConfig: HarnessConfig = {
  onboardingComplete: true,
  harnessHome: 'C:/hive',
  registeredRepos: ['C:/repo-a'],
  autoMode: false,
  defaultCommand: 'claude',
  semanticMemory: false,
  embeddingModel: 'minilm'
};

function renderModal() {
  render(<AddAgentModal onClose={() => {}} config={baseConfig} onConfigChange={() => {}} />);
}

async function spawnWith(opts: { checkShardx?: boolean }) {
  // Name lives on Identity (the default section); Workspace has the project
  // path AND the mcpExtras checkboxes.
  fireEvent.change(screen.getByPlaceholderText('Ada'), { target: { value: 'toko-bot' } });
  fireEvent.click(screen.getByText('Workspace'));
  if (opts.checkShardx) {
    fireEvent.click(screen.getByText('ShardX Browser'));
  }
  fireEvent.change(screen.getByPlaceholderText('/path/to/your/project'), {
    target: { value: 'C:/repo-a' }
  });
  fireEvent.click(screen.getByText('spawn'));
}

describe('AddAgentModal MCP extras', () => {
  let cth: FakeBridge;
  beforeEach(() => {
    cth = installFakeBridge();
    // Overrides bypass the proxy's call recorder — record explicitly.
    cth.spawnPty = async (req: unknown) => {
      cth.calls.push({ method: 'spawnPty', args: [req] });
      return { ok: true, cwd: 'C:/repo-a' };
    };
    useStore.setState({ hireQueue: { pending: [], reviewed: 0 } } as never);
  });

  it('checking ShardX Browser arms mcpEnable on the spawn payload', async () => {
    // Gates green → the consent prompt is skipped and shardx rides through.
    cth.shardxStatus = async () => ({
      launcherRunning: true, mcpInstalled: true, tokenSet: true
    });
    renderModal();
    await spawnWith({ checkShardx: true });
    await waitFor(() => {
      const call = cth.calls.find((c) => c.method === 'spawnPty');
      expect(call).toBeTruthy();
      expect((call!.args[0] as { mcpEnable?: string[] }).mcpEnable).toEqual(['shardx']);
    });
  });

  it('no extras checked → mcpEnable stays absent (global consent untouched)', async () => {
    renderModal();
    await spawnWith({});
    await waitFor(() => {
      const call = cth.calls.find((c) => c.method === 'spawnPty');
      expect(call).toBeTruthy();
      expect((call!.args[0] as { mcpEnable?: string[] }).mcpEnable).toBeUndefined();
    });
  });

  // Consent flow: shardx armed + the local chain incomplete → one yes/no is
  // the only user step; yes runs the zero-touch bootstrap then spawns armed.
  it('shardx armed + gates red + consent YES → bootstrap runs, spawn keeps shardx', async () => {
    cth.shardxStatus = async () => {
      cth.calls.push({ method: 'shardxStatus', args: [] });
      return { launcherRunning: false, mcpInstalled: false, tokenSet: false };
    };
    cth.shardxBootstrap = async () => {
      cth.calls.push({ method: 'shardxBootstrap', args: [] });
      return { ok: true, steps: {} };
    };
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderModal();
    await spawnWith({ checkShardx: true });
    await waitFor(() => {
      expect(confirm).toHaveBeenCalled();
      expect(cth.calls.some((c) => c.method === 'shardxBootstrap')).toBe(true);
      const call = cth.calls.find((c) => c.method === 'spawnPty');
      expect((call!.args[0] as { mcpEnable?: string[] }).mcpEnable).toEqual(['shardx']);
    });
    confirm.mockRestore();
  });

  it('consent NO → agent still spawns, shardx stripped from mcpEnable', async () => {
    cth.shardxStatus = async () => {
      cth.calls.push({ method: 'shardxStatus', args: [] });
      return { launcherRunning: false, mcpInstalled: false, tokenSet: false };
    };
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderModal();
    await spawnWith({ checkShardx: true });
    await waitFor(() => {
      const call = cth.calls.find((c) => c.method === 'spawnPty');
      expect(call).toBeTruthy();
      expect((call!.args[0] as { mcpEnable?: string[] }).mcpEnable).toBeUndefined();
      expect(cth.calls.some((c) => c.method === 'shardxBootstrap')).toBe(false);
    });
    confirm.mockRestore();
  });

  it('gates already green → no consent prompt, spawn arms shardx directly', async () => {
    cth.shardxStatus = async () => {
      cth.calls.push({ method: 'shardxStatus', args: [] });
      return { launcherRunning: true, mcpInstalled: true, tokenSet: true };
    };
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderModal();
    await spawnWith({ checkShardx: true });
    await waitFor(() => {
      const call = cth.calls.find((c) => c.method === 'spawnPty');
      expect((call!.args[0] as { mcpEnable?: string[] }).mcpEnable).toEqual(['shardx']);
    });
    expect(confirm).not.toHaveBeenCalled();
    confirm.mockRestore();
  });
});
