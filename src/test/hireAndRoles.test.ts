/**
 * hire manifest validation + deep-link gate + broadcast targets +
 * agentRole — the shareable-hire security surface and role durability.
 */
import { describe, it, expect } from 'vitest';
import {
  validateHireManifest, parseHireDeepLink, isAllowedManifestUrl, HIRE_SPEC_V1
} from '@/shared/hire';
import { selectBroadcastTargets } from '@/shared/broadcast';
import { isDurableRole, preferredAgentRole, roleForHiveSpawn } from '@/shared/agentRole';

describe('validateHireManifest', () => {
  const base = { spec: HIRE_SPEC_V1, name: 'Reviewer' };

  it('accepts a minimal valid manifest', () => {
    const r = validateHireManifest(base);
    expect(r.ok).toBe(true);
  });

  it('rejects non-objects and wrong spec', () => {
    expect(validateHireManifest('x').ok).toBe(false);
    expect(validateHireManifest(null).ok).toBe(false);
    expect(validateHireManifest([1, 2]).ok).toBe(false);
    expect(validateHireManifest({ spec: 'v2', name: 'x' }).ok).toBe(false);
  });

  it('name is required; a missing name fails', () => {
    expect(validateHireManifest({ spec: HIRE_SPEC_V1 }).ok).toBe(false);
  });

  it('provider must be a known id ("agy" normalises to antigravity)', () => {
    const ok = validateHireManifest({ ...base, provider: 'agy' });
    expect(ok.ok).toBe(true);
    if (ok.ok) expect(ok.manifest?.provider).toBe('antigravity');
    expect(validateHireManifest({ ...base, provider: 'zzz' }).ok).toBe(false);
  });

  it('commandFlags is default-deny: only safe flags, no flag-smuggling values', () => {
    // A safe flag with a value.
    const good = validateHireManifest({ ...base, commandFlags: ['--model', 'opus'] });
    expect(good.ok).toBe(true);
    // An unsafe flag token.
    const bad = validateHireManifest({ ...base, commandFlags: ['--dangerously-skip-permissions'] });
    expect(bad.ok).toBe(false);
    // First token must be flag-shaped.
    expect(validateHireManifest({ ...base, commandFlags: ['value-first'] }).ok).toBe(false);
    // A value may not smuggle a second flag.
    expect(validateHireManifest({ ...base, commandFlags: ['--model', 'opus', 'evil'] }).ok).toBe(false);
  });

  it('providers the floor cannot spawn reject outright', () => {
    // Shared hires are curated to engines the app provably drives.
    expect(validateHireManifest({ ...base, provider: 'hermes' }).ok).toBe(false);
    expect(validateHireManifest({ ...base, provider: 'codex' }).ok).toBe(true);
  });
});

describe('hire deep-link gate', () => {
  it('only the munderdifflin://hire?src= link shape parses', () => {
    expect(parseHireDeepLink('munderdifflin://hire?src=https://ok.example/m.json'))
      .toBe('https://ok.example/m.json');
    expect(parseHireDeepLink('munderdifflin:hire?src=https://ok.example/m.json'))
      .toBe('https://ok.example/m.json');
    expect(parseHireDeepLink('javascript:alert(1)')).toBeNull();
    expect(parseHireDeepLink('munderdifflin://not-hire?src=https://x/m.json')).toBeNull();
    expect(parseHireDeepLink('munderdifflin://hire')).toBeNull(); // no src
  });
  it('manifest URLs are https-only outside loopback — file:// etc. never fetch', () => {
    expect(isAllowedManifestUrl(new URL('https://x.com/m.json'))).toBe(true);
    expect(isAllowedManifestUrl(new URL('http://localhost:8080/m.json'))).toBe(true);
    expect(isAllowedManifestUrl(new URL('http://evil.com/m.json'))).toBe(false);
    expect(isAllowedManifestUrl(new URL('file:///etc/passwd'))).toBe(false);
    expect(isAllowedManifestUrl(new URL('javascript:alert(1)'))).toBe(false);
  });
});

describe('selectBroadcastTargets', () => {
  it('excludes sender, assistant, and archived — includes everyone else', () => {
    const agents = {
      god: {},
      sender: {},
      assistant: { isAssistant: true },
      gone: { archived: true },
      worker1: {},
      worker2: {},
    };
    const targets = selectBroadcastTargets(agents, 'sender');
    expect(targets).toEqual(expect.arrayContaining(['god', 'worker1', 'worker2']));
    expect(targets).not.toContain('sender');
    expect(targets).not.toContain('assistant');
    expect(targets).not.toContain('gone');
  });
});

describe('agentRole durability', () => {
  it('status-like captions are not durable roles', () => {
    expect(isDurableRole('backend reviewer')).toBe(true);
    for (const s of ['idle', 'paused', 'on standby', 'starting up', 'reconnecting']) {
      expect(isDurableRole(s), s).toBe(false);
    }
    expect(isDurableRole('')).toBe(false);
    expect(isDurableRole(null)).toBe(false);
  });

  it('preferredAgentRole: durable beats caption; god falls back to orchestrator', () => {
    expect(preferredAgentRole('idle', 'QA lead')).toBe('QA lead');
    expect(preferredAgentRole('new role', 'old role')).toBe('new role');
    expect(preferredAgentRole(undefined, undefined, true)).toBe('orchestrator (god)');
    expect(preferredAgentRole(undefined, undefined, false)).toBe('agent');
  });

  it('roleForHiveSpawn omits transient captions so registry keeps the real role', () => {
    expect(roleForHiveSpawn({ description: 'idle' })).toBeUndefined();
    expect(roleForHiveSpawn({ description: 'backend dev' })).toBe('backend dev');
    expect(roleForHiveSpawn({ isGod: true, description: 'idle' })).toBe('orchestrator (god)');
  });
});
