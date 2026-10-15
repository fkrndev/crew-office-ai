import { describe, expect, it } from 'vitest';
import { audienceBrief, audienceChangeMessage } from '@shared/audience';
import { initialGodPrompt } from '@/hooks/useHive';

describe('audience register', () => {
  it('only a non-technical human adds an instruction to the boot prompt', () => {
    expect(audienceBrief('technical')).toBeNull();
    expect(audienceBrief(undefined)).toBeNull();
    expect(audienceBrief('non-technical')).toContain('not technical');
  });

  it('the god boot prompt carries the plain-language rule when asked', () => {
    const base = initialGodPrompt('Michael');
    expect(base).not.toContain('not technical');
    const plain = initialGodPrompt('Michael', 'non-technical');
    expect(plain.startsWith(base)).toBe(true);
    expect(plain).toContain('plain everyday language');
    expect(initialGodPrompt('Michael', 'technical')).toBe(base);
  });

  it('a live change tells god which register to use, in both directions', () => {
    const toPlain = audienceChangeMessage('non-technical');
    expect(toPlain.subject).toMatch(/PREFERENCE/);
    expect(toPlain.body).toContain('not technical');
    const toTech = audienceChangeMessage('technical');
    expect(toTech.body).toContain('is technical');
    expect(toTech.body).not.toContain('not technical');
  });
});
