/**
 * Settings → MCP "ShardX browser setup" card — the only UI surface that reaches
 * cth.shardxStatus / shardxSetup / shardxSetToken / shardxDownloadLauncher.
 * Without it the catalog toggle armed <harness>/mcp-servers/shardx/index.js
 * that nothing ever vendored — a dead MCP path.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { McpDefaultsSettings } from '@/components/McpDefaultsSettings';
import type { HarnessConfig } from '@/store/config';

const baseConfig: HarnessConfig = {
  onboardingComplete: true,
  harnessHome: 'C:/hive',
  registeredRepos: [],
  autoMode: false,
  defaultCommand: 'claude',
  semanticMemory: false,
  embeddingModel: 'minilm'
};

function renderSettings(cth: FakeBridge) {
  // Overrides bypass the proxy's call recorder — record explicitly.
  cth.shardxStatus = async () => {
    cth.calls.push({ method: 'shardxStatus', args: [] });
    return {
      launcherRunning: false,
      mcpInstalled: false,
      tokenSet: false,
      api: 'http://127.0.0.1:40325',
      mcpDir: 'C:/hive/mcp-servers/shardx'
    };
  };
  render(<McpDefaultsSettings config={baseConfig} />);
}

describe('McpDefaultsSettings ShardX setup card', () => {
  let cth: FakeBridge;
  beforeEach(() => {
    cth = installFakeBridge();
  });

  it('polls shardxStatus and renders the three gates', async () => {
    renderSettings(cth);
    await waitFor(() => {
      expect(cth.calls.some((c) => c.method === 'shardxStatus')).toBe(true);
    });
    expect(screen.getByText('Launcher running')).toBeTruthy();
    expect(screen.getByText('MCP bridge installed')).toBeTruthy();
    expect(screen.getByText('API token saved')).toBeTruthy();
  });

  it('Install bridge calls shardxSetup then re-polls status', async () => {
    renderSettings(cth);
    await waitFor(() => screen.getByText('Install bridge'));
    fireEvent.click(screen.getByText('Install bridge'));
    await waitFor(() => {
      expect(cth.calls.some((c) => c.method === 'shardxSetup')).toBe(true);
    });
    await waitFor(() => {
      const polls = cth.calls.filter((c) => c.method === 'shardxStatus').length;
      expect(polls).toBeGreaterThanOrEqual(2);
    });
  });

  it('Auto setup calls shardxBootstrap — the one-consent full chain', async () => {
    renderSettings(cth);
    await waitFor(() => screen.getByText('Auto setup'));
    fireEvent.click(screen.getByText('Auto setup'));
    await waitFor(() => {
      expect(cth.calls.some((c) => c.method === 'shardxBootstrap')).toBe(true);
    });
  });

  it('Download launcher calls shardxDownloadLauncher', async () => {
    renderSettings(cth);
    await waitFor(() => screen.getByText('Download launcher'));
    fireEvent.click(screen.getByText('Download launcher'));
    await waitFor(() => {
      expect(cth.calls.some((c) => c.method === 'shardxDownloadLauncher')).toBe(true);
    });
  });

  it('Save token passes the trimmed token to shardxSetToken and clears the field', async () => {
    renderSettings(cth);
    const input = await waitFor(() =>
      screen.getByPlaceholderText('Automation API token')
    ) as HTMLInputElement;
    fireEvent.change(input, { target: { value: '  tok-abc  ' } });
    fireEvent.click(screen.getByText('Save token'));
    await waitFor(() => {
      const call = cth.calls.find((c) => c.method === 'shardxSetToken');
      expect(call).toBeTruthy();
      expect(call!.args[0]).toBe('tok-abc');
    });
    await waitFor(() => expect(input.value).toBe(''));
  });
});
