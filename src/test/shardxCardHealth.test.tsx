/**
 * ShardX card health + proxy binding regression.
 *
 * The empty-fingerprint-library failure used to hide inside the launcher —
 * profiles couldn't be created and nothing on the card said why. The card
 * now shows live counts (fingerprints/profiles/running) with a red warning
 * on an empty library, and each profile row carries a proxy select that
 * binds through shardxSetProfileProxy with an optimistic update.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge, type FakeBridge } from './fakeBridge';
import { McpDefaultsSettings } from '@/components/McpDefaultsSettings';
import type { HarnessConfig } from '@/store/config';

let cth: FakeBridge;
const config = { mcpDefaults: {} } as unknown as HarnessConfig;

const healthyStatus = {
  launcherRunning: true,
  mcpInstalled: true,
  tokenSet: true,
  fingerprintCount: 220,
  profileCount: 1,
  runningCount: 0
};

beforeEach(() => {
  cth = installFakeBridge({
    shardxStatus: async () => healthyStatus,
    shardxProfiles: async () => ({ ok: true, profiles: [] }),
    shardxProxies: async () => ({ ok: true, proxies: [] }),
    shardxTokenPreview: async () => ({ ok: false }),
    shardxSetProfileProxy: vi.fn(async () => ({ ok: true }))
  });
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ShardX live counts', () => {
  it('shows fingerprint/profile/running counts from status', async () => {
    render(<McpDefaultsSettings config={config} />);
    await waitFor(() =>
      expect(
        screen.getByText(/fingerprints 220.*profiles 1.*running 0/i)
      ).toBeTruthy()
    );
  });

  it('warns when the fingerprint library is empty', async () => {
    cth.shardxStatus = vi.fn(async () => ({
      ...healthyStatus,
      fingerprintCount: 0
    }));
    render(<McpDefaultsSettings config={config} />);
    await waitFor(() =>
      expect(screen.getByText(/fingerprint library empty/i)).toBeTruthy()
    );
  });

  it('hides the counts line when the API is unreachable', async () => {
    cth.shardxStatus = vi.fn(async () => ({
      launcherRunning: false,
      mcpInstalled: true,
      tokenSet: false
    }));
    render(<McpDefaultsSettings config={config} />);
    // Let the refresh promise settle, then assert nothing rendered.
    await waitFor(() => expect(cth.shardxStatus).toHaveBeenCalled());
    expect(screen.queryByText(/fingerprints \d+/i)).toBeNull();
  });
});

describe('ShardX proxy binding', () => {
  const profiles = [
    { id: 'p1', name: 'office-toko', proxy_id: 'px-1', running: false, auto: true },
    { id: 'p2', name: 'shopee-main', proxy_id: null, running: true, auto: false }
  ];
  const proxies = [
    { id: 'px-1', name: 'sg-1', kind: 'socks5', host: '1.2.3.4', port: 1080, country: 'SG' },
    { id: 'px-2', name: 'us-1', kind: 'http', host: '5.6.7.8', port: 8080, country: 'US' }
  ];

  beforeEach(() => {
    cth.shardxProfiles = vi.fn(async () => ({ ok: true, profiles }));
    cth.shardxProxies = vi.fn(async () => ({ ok: true, proxies }));
  });

  // The profile name is a nested <span> inside the row <span> — closest()
  // returns the name span itself, so climb one more level to the row.
  const selectFor = (name: string) =>
    screen.getByText(name).closest('span')!.parentElement!.querySelector('select')!;

  it('renders each profile bound to its current proxy', async () => {
    render(<McpDefaultsSettings config={config} />);
    await waitFor(() => expect(selectFor('office-toko')).toBeTruthy());
    expect((selectFor('office-toko') as HTMLSelectElement).value).toBe('px-1');
    expect((selectFor('shopee-main') as HTMLSelectElement).value).toBe('');
  });

  it('binds a proxy through shardxSetProfileProxy and updates in place', async () => {
    render(<McpDefaultsSettings config={config} />);
    // Profiles land async via refresh() — wait for the row, then grab it.
    await waitFor(() => expect(selectFor('shopee-main')).toBeTruthy());
    const sel = selectFor('shopee-main');
    fireEvent.change(sel, { target: { value: 'px-2' } });
    await waitFor(() =>
      expect(cth.shardxSetProfileProxy).toHaveBeenCalledWith('p2', 'px-2')
    );
    // Optimistic: the row shows the new binding without a full refetch.
    expect((selectFor('shopee-main') as HTMLSelectElement).value).toBe('px-2');
  });

  it('an empty choice unbinds the proxy', async () => {
    render(<McpDefaultsSettings config={config} />);
    await waitFor(() => expect(selectFor('office-toko')).toBeTruthy());
    fireEvent.change(selectFor('office-toko'), { target: { value: '' } });
    await waitFor(() =>
      expect(cth.shardxSetProfileProxy).toHaveBeenCalledWith('p1', '')
    );
  });

  it('a failed bind restores the row from the backend', async () => {
    cth.shardxSetProfileProxy = vi.fn(async () => ({ ok: false, error: 'proxy dead' }));
    render(<McpDefaultsSettings config={config} />);
    await waitFor(() => expect(selectFor('shopee-main')).toBeTruthy());
    fireEvent.change(selectFor('shopee-main'), { target: { value: 'px-2' } });
    await waitFor(() => expect(screen.getByText(/proxy dead/i)).toBeTruthy());
    // refresh() re-read the profiles → the row snaps back to the real bind.
    await waitFor(() =>
      expect((selectFor('shopee-main') as HTMLSelectElement).value).toBe('')
    );
  });
});
