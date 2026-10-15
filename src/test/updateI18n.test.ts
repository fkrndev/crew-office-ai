/**
 * The signed-updater + hive-degraded strings exist, with identical key sets and
 * the same {{placeholders}}, in all four locales.
 */
import { describe, it, expect } from 'vitest';
import en from '@/i18n/locales/en.json';
import ar from '@/i18n/locales/ar.json';
import id from '@/i18n/locales/id.json';
import zh from '@/i18n/locales/zh-CN.json';

const locales = { en, ar, id, 'zh-CN': zh } as Record<string, any>;

const flatten = (o: any, prefix = ''): Record<string, string> =>
  Object.entries(o).reduce<Record<string, string>>((acc, [k, v]) => {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object') Object.assign(acc, flatten(v, key));
    else acc[key] = String(v);
    return acc;
  }, {});

const placeholders = (s: string) => [...s.matchAll(/{{\s*(\w+)\s*}}/g)].map((m) => m[1]).sort();

describe('updater / degraded i18n', () => {
  const sections = ['updateToast', 'updatesSection', 'notice.hiveDegraded'];
  const pick = (loc: any) => {
    const flat = flatten(loc);
    return Object.fromEntries(
      Object.entries(flat).filter(([k]) => sections.some((s) => k === s || k.startsWith(`${s}.`)))
    );
  };

  it('every locale has the same keys as English', () => {
    const base = pick(en);
    expect(Object.keys(base)).toEqual(expect.arrayContaining([
      'updateToast.availableBody', 'updateToast.downloading', 'updateToast.downloadUpdate',
      'updateToast.downloadingBody', 'updateToast.downloadingShort', 'updateToast.manualBodyReason',
      'notice.hiveDegraded.proxyBind', 'notice.hiveDegraded.generic'
    ]));
    for (const [name, loc] of Object.entries(locales)) {
      expect(Object.keys(pick(loc)).sort(), name).toEqual(Object.keys(base).sort());
    }
  });

  it('translations keep the same placeholders and are not blank', () => {
    const base = pick(en);
    for (const [name, loc] of Object.entries(locales)) {
      const flat = pick(loc);
      for (const [k, v] of Object.entries(base)) {
        expect(flat[k].trim().length, `${name}:${k}`).toBeGreaterThan(0);
        expect(placeholders(flat[k]), `${name}:${k}`).toEqual(placeholders(v));
      }
    }
  });
});
