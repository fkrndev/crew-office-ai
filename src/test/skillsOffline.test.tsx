/**
 * Offline skills — the bundled catalog tab in SkillsTab. No network: the list
 * comes from resources/skills inside the app, and install copies the folder
 * straight into ~/.claude/skills.
 */
import { describe, it, expect } from 'vitest';
import { vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { installFakeBridge } from './fakeBridge';
import { SkillsTab } from '@/components/SkillsTab';

const bundled = [
  {
    name: 'meta-ads',
    dirName: 'meta-ads',
    description: 'Meta advertising strategy pack',
    category: 'ads',
    installed: false
  },
  {
    name: 'ticket-triage',
    dirName: 'ticket-triage',
    description: 'Sort support tickets by urgency',
    category: 'support',
    installed: true
  }
];

describe('SkillsTab offline catalog', () => {
  it('lists bundled skills without touching the network catalog', async () => {
    const cth = installFakeBridge({ skillsBundled: async () => bundled });
    render(<SkillsTab />);

    fireEvent.click(screen.getByText('offline'));

    await waitFor(() => expect(screen.getByText('META-ADS')).toBeTruthy());
    expect(screen.getByText('TICKET-TRIAGE')).toBeTruthy();
    // The ONLINE catalog was never fetched.
    expect(cth.calls.every((c) => c.method !== 'skillsCatalog')).toBe(true);
  });

  it('install calls skillsInstallBundled with the dir name and flips to installed', async () => {
    let installedDir: string | undefined;
    let localReloads = 0;
    void installFakeBridge({
      skillsBundled: async () => bundled,
      skillsInstallBundled: async (dirName: string) => {
        installedDir = dirName;
        return { ok: true, path: `C:/u/.claude/skills/${dirName}` };
      },
      skillsLocal: async () => { localReloads++; return []; }
    });
    render(<SkillsTab />);

    fireEvent.click(screen.getByText('offline'));
    await waitFor(() => expect(screen.getByText('META-ADS')).toBeTruthy());

    fireEvent.click(screen.getByText('install'));

    await waitFor(() => expect(installedDir).toBe('meta-ads'));
    // The installed list reloads so the Installed tab reflects it.
    await waitFor(() => expect(localReloads).toBeGreaterThan(0));
  });

  it('already-installed rows show Installed, not an Install button', async () => {
    installFakeBridge({ skillsBundled: async () => bundled });
    render(<SkillsTab />);

    fireEvent.click(screen.getByText('offline'));
    await waitFor(() => expect(screen.getByText('TICKET-TRIAGE')).toBeTruthy());

    // One install button only — for the not-yet-installed row.
    expect(screen.getAllByText('install')).toHaveLength(1);
    expect(screen.getAllByText('Installed').length).toBeGreaterThan(0);
  });

  it('category filter narrows the offline list', async () => {
    installFakeBridge({ skillsBundled: async () => bundled });
    render(<SkillsTab />);

    fireEvent.click(screen.getByText('offline'));
    await waitFor(() => expect(screen.getByText('META-ADS')).toBeTruthy());

    fireEvent.change(screen.getByTitle('Categories as published by the catalog'), {
      target: { value: 'support' }
    });

    expect(screen.queryByText('META-ADS')).toBeNull();
    expect(screen.getByText('TICKET-TRIAGE')).toBeTruthy();
  });

  it('failed install surfaces the error inline', async () => {
    installFakeBridge({
      skillsBundled: async () => bundled,
      skillsInstallBundled: async () => ({ ok: false, error: 'not a bundled skill' })
    });
    render(<SkillsTab />);

    fireEvent.click(screen.getByText('offline'));
    await waitFor(() => expect(screen.getByText('META-ADS')).toBeTruthy());
    fireEvent.click(screen.getByText('install'));

    await waitFor(() => expect(screen.getByText('not a bundled skill')).toBeTruthy());
  });
});
