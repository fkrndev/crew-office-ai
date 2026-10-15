/**
 * updateState — the version/status state machine behind UpdateBadge (the
 * component whose crash blanked the app). Pins semver compare, the pending-
 * version rule, and the "don't wipe a staged update" reducer.
 */
import { describe, it, expect } from 'vitest';
import {
  parseVersion, isNewer, pendingVersion, shouldShowReleaseDrop,
  clampPercent, reduceStatus, installerUrl, manualDownloadUrl
} from '@/shared/updateState';
import type { UpdateStatus } from '@/shared/updateState';

describe('parseVersion / isNewer', () => {
  it('accepts v-prefixed semver and rejects junk', () => {
    expect(parseVersion('1.2.3')).toEqual([1, 2, 3]);
    expect(parseVersion('v0.4.7')).toEqual([0, 4, 7]);
    expect(parseVersion('nope')).toBeNull();
  });
  it('compares numerically — 0.10 > 0.9', () => {
    expect(isNewer('0.10.0', '0.9.9')).toBe(true);
    expect(isNewer('0.9.0', '0.9.0')).toBe(false);
    expect(isNewer('junk', '0.9.0')).toBe(false); // unparseable never wins
  });
});

describe('pendingVersion / shouldShowReleaseDrop', () => {
  it('only a genuinely newer staged version is pending', () => {
    const st = { state: 'available', version: '0.4.7' } as UpdateStatus;
    expect(pendingVersion(st, '0.4.6')).toBe('0.4.7');
    expect(pendingVersion(st, '0.4.7')).toBeNull();
    expect(pendingVersion({ state: 'just-updated', version: '0.4.7' } as UpdateStatus, '0.4.6')).toBeNull();
    expect(pendingVersion(null, '0.4.6')).toBeNull();
  });
  it('release drop shows on first run and upgrades, not same-version relaunch', () => {
    expect(shouldShowReleaseDrop(null, '0.4.7')).toBe(true);       // fresh install
    expect(shouldShowReleaseDrop('0.4.6', '0.4.7')).toBe(true);    // upgrade
    expect(shouldShowReleaseDrop('0.4.7', '0.4.7')).toBe(false);   // seen
    expect(shouldShowReleaseDrop('0.5.0', '0.4.7')).toBe(false);   // downgrade
  });
});

describe('reduceStatus', () => {
  it('a newer release always wins; a same-version re-check does not regress', () => {
    const downloaded = { state: 'downloaded', version: '0.4.7' } as UpdateStatus;
    const checking = { state: 'checking' } as UpdateStatus;
    // A re-check while 0.4.7 is staged must NOT wipe "restart to update".
    expect(reduceStatus(downloaded, checking)).toBe(downloaded);
    // But a genuinely newer release supersedes the staged one.
    const newer = { state: 'available', version: '0.4.8' } as UpdateStatus;
    expect(reduceStatus(downloaded, newer)).toBe(newer);
  });

  it('signed flow advances available -> downloading -> downloaded', () => {
    const available = { state: 'available', version: '0.4.8', signed: true } as UpdateStatus;
    const downloading = { state: 'downloading', version: '0.4.8', percent: 10 } as UpdateStatus;
    const downloaded = { state: 'downloaded', version: '0.4.8' } as UpdateStatus;
    expect(reduceStatus(available, downloading)).toBe(downloading);
    expect(reduceStatus(downloading, { state: 'downloading', version: '0.4.8', percent: 11 } as UpdateStatus).state).toBe('downloading');
    // (main never re-checks mid-download, so `available` after `downloading`
    // only ever means "the download failed and may be retried")
    expect(reduceStatus(downloading, downloaded)).toBe(downloaded);
  });

  it('a failed download (signature rejected) replaces downloading instead of sticking at 100%', () => {
    const downloading = { state: 'downloading', version: '0.4.8', percent: 100 } as UpdateStatus;
    const failed = { state: 'available-manual', version: '0.4.8', url: 'https://github.com/fkrndev/crew-office-ai/releases/tag/v0.4.8', reason: 'signature mismatch' } as UpdateStatus;
    expect(reduceStatus(downloading, failed)).toBe(failed);
    const err = { state: 'error', message: 'boom' } as UpdateStatus;
    expect(reduceStatus(downloading, err)).toBe(err);
    // ... but a staged download is still protected from re-check noise
    const downloaded = { state: 'downloaded', version: '0.4.8' } as UpdateStatus;
    expect(reduceStatus(downloaded, err)).toBe(downloaded);
  });
});

describe('clampPercent / installerUrl', () => {
  it('clamps out-of-range and non-finite download percents', () => {
    expect(clampPercent(-3)).toBe(0);
    expect(clampPercent(104)).toBe(100);
    expect(clampPercent(NaN)).toBe(0);
    expect(clampPercent(42.6)).toBe(43);
  });
  it('builds the right asset name per platform; manualDownloadUrl honours downloadUrl', () => {
    expect(installerUrl('0.4.7', 'win32', 'x64'))
      .toBe('https://github.com/fkrndev/crew-office-ai/releases/download/v0.4.7/Agent.Office_0.4.7_x64-setup.exe');
    expect(installerUrl('v0.4.7', 'darwin', 'arm64')).toContain('Agent.Office_0.4.7_aarch64.dmg');
    expect(installerUrl('0.4.7', 'linux', 'x64')).toContain('Agent.Office_0.4.7_amd64.AppImage');
    const st = { state: 'available-manual', version: '0.4.7', downloadUrl: 'https://x/custom' } as UpdateStatus;
    expect(manualDownloadUrl(st, 'win32', 'x64')).toBe('https://x/custom');
  });
});
