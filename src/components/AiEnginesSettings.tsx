import { useState, useEffect, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import type { HarnessConfig, AgentProvider } from '@/store/config';
import { PixelButton } from './PixelButton';
import { ProviderLogo } from './ProviderLogo';
import { useStore } from '@/store/store';
import { ErrorNotice } from './FriendlyError';

/**
 * AiEnginesSettings — the v0.3.1 per-provider config surface for the BYOK CLI
 * engines (OpenCode · Crush · pi.dev · Qwen). Two stores by what the datum is:
 *  - API keys → WRITE-ONLY in the secret broker (`providerKey:*` IPC). Keyed by the
 *    BACKEND model-provider (anthropic/openai/…). The field shows only set/not-set;
 *    the plaintext is never read back to the renderer (materialized MAIN-only at spawn).
 *  - Local base-URL + default model → HarnessConfig (`providerBaseUrls` /
 *    `providerDefaultModels`), keyed by CLI provider. Non-secret; normal config save.
 * See hive/shared/cli-agents/settings-ui-schema.md.
 */

/** Backend model-providers whose keys the CLIs read from standard env vars. Must
 *  match BACKEND_KEY_ENV in src/main/index.ts. */
export const BACKENDS: Array<{ id: string; label: string; envVar: string }> = [
  // The two keys the default engine (Hermes) runs on cheaply lead the list.
  { id: 'openrouter', label: 'OpenRouter', envVar: 'OPENROUTER_API_KEY' },
  // OpenAI-compatible — the key reaches agents as DEEPSEEK_API_KEY (+ OPENAI_
  // API_KEY, which most OpenAI-compatible CLIs read). Pair it with a base URL
  // override (https://api.deepseek.com) on the CLI provider below.
  { id: 'deepseek', label: 'DeepSeek', envVar: 'DEEPSEEK_API_KEY' },
  { id: 'anthropic', label: 'Anthropic', envVar: 'ANTHROPIC_API_KEY' },
  { id: 'openai', label: 'OpenAI', envVar: 'OPENAI_API_KEY' },
  { id: 'google', label: 'Google · Gemini', envVar: 'GEMINI_API_KEY' },
  { id: 'groq', label: 'Groq', envVar: 'GROQ_API_KEY' }
];

/** CLI engines that take a per-provider local base-URL + default model. `hint`
 *  values are technical endpoint descriptions — kept English (technical data). */
export const CLIS: Array<{ id: AgentProvider; label: string; hint: string; hintKey?: string }> = [
  // Hermes reads OPENAI_BASE_URL natively — pointing it at api.deepseek.com
  // (+ a DeepSeek key above) makes a Hermes worker talk DeepSeek.
  // The hint is an i18n key: the one-liner used to say only "exported as
  // OPENAI_BASE_URL", which tells a Hermes user nothing about when it applies
  // (answer: only when ~/.hermes has no provider of its own). Hermes is the
  // default engine, so it leads the list.
  { id: 'hermes', label: 'Hermes', hint: 'OpenAI-compatible endpoint — exported as OPENAI_BASE_URL (e.g. https://api.deepseek.com)', hintKey: 'aiEngines.hermesHint' },
  { id: 'opencode', label: 'OpenCode', hint: 'http://localhost:11434/v1 (Ollama) — injected as a local provider' },
  { id: 'crush', label: 'Crush', hint: 'OpenAI-compatible endpoint — used as the proxy upstream' },
  { id: 'pi', label: 'Pi', hint: 'local models are file-based (models.json); base-URL reserved' },
  { id: 'qwen', label: 'Qwen', hint: 'OpenAI-compatible endpoint — used as the proxy upstream' },
  // OpenClaw honors the same OPENAI_BASE_URL reroute on its openai-compat
  // route; provider keys ride in via the API-key section above.
  { id: 'openclaw', label: 'OpenClaw', hint: 'OpenAI-compatible endpoint — exported as OPENAI_BASE_URL' }
];

export const inputStyle: CSSProperties = {
  width: '100%',
  padding: '6px 8px 4px',
  background: 'var(--cth-paper-100)',
  border: 'none',
  boxShadow: 'inset 0 0 0 1px var(--cth-ink-100)',
  fontFamily: 'var(--cth-font-ui)',
  fontSize: 13,
  color: 'var(--cth-ink-900)',
  outline: 'none'
};
export const labelStyle: CSSProperties = {
  fontFamily: 'var(--cth-font-display)',
  fontSize: 8,
  lineHeight: '12px',
  color: 'var(--cth-ink-700)',
  textTransform: 'uppercase'
};
const headStyle: CSSProperties = {
  fontFamily: 'var(--cth-font-display)', fontSize: 8, lineHeight: '12px',
  color: 'var(--cth-ink-500)', textTransform: 'uppercase', marginBottom: 2
};

/** Write-only provider API keys: which backends already have one stored (boolean
 *  only — never the value), the drafts being typed, and the save / remove calls.
 *  Shared by the full engines list below and the one-field "AI connection" tab of
 *  simple mode, so both talk to the same secret broker the same way. */
export function useProviderKeys() {
  const { t } = useTranslation();
  // Keep the global "OpenAI key present" signal (boolean only) live so the Talk
  // button's missing-key warning clears the instant the user saves their OpenAI key
  // here — without it the gate only refreshes on next app start. apikey:openai is
  // the same key the Realtime mint reads; saving/clearing it flips the gate.
  const setHasOpenAiKey = useStore((s) => s.setHasOpenAiKey);
  // Which backends already have a key stored (boolean only — never the value).
  const [hasKey, setHasKey] = useState<Record<string, boolean>>({});
  const [draftKey, setDraftKey] = useState<Record<string, string>>({});
  const [note, setNote] = useState<Record<string, string>>({});
  // The raw error behind a failed save (simple mode shows it in plain words).
  const [noteRaw, setNoteRaw] = useState<Record<string, unknown>>({});
  const setFail = (backend: string, raw: unknown, text: string) => {
    setNote((s) => ({ ...s, [backend]: text }));
    setNoteRaw((s) => ({ ...s, [backend]: raw }));
  };

  // Reseed set/not-set flags on mount (write-only — only the boolean is fetched).
  useEffect(() => {
    let alive = true;
    (async () => {
      const out: Record<string, boolean> = {};
      for (const b of BACKENDS) {
        try { out[b.id] = await window.cth.providerKeyHas(b.id); } catch { out[b.id] = false; }
      }
      if (alive) setHasKey(out);
    })();
    return () => { alive = false; };
  }, []);

  const saveKey = async (backend: string) => {
    const key = (draftKey[backend] ?? '').trim();
    if (!key) return;
    setNoteRaw((s) => { const n = { ...s }; delete n[backend]; return n; });
    try {
      const r = await window.cth.providerKeySet({ backend, key });
      if (r.ok) {
        setHasKey((s) => ({ ...s, [backend]: true }));
        setDraftKey((s) => ({ ...s, [backend]: '' }));
        setNote((s) => ({ ...s, [backend]: t('aiEngines.saved') }));
        // OpenAI key gates Talk — mirror presence to the store so the warning clears now.
        if (backend === 'openai') setHasOpenAiKey(true);
        // A saved key answers a "needs an API key" spawn — drop the call to action.
        useStore.getState().setEngineNeedsKey(false);
      } else setFail(backend, r.error ?? t('aiEngines.failed'), r.error ?? t('aiEngines.failed'));
    } catch (e) { setFail(backend, e, e instanceof Error ? e.message : String(e)); }
  };
  const clearKey = async (backend: string) => {
    try {
      await window.cth.providerKeyClear(backend);
      setHasKey((s) => ({ ...s, [backend]: false }));
      setNote((s) => ({ ...s, [backend]: t('aiEngines.cleared') }));
      // OpenAI key gates Talk — clearing it disables Talk; reflect that immediately.
      if (backend === 'openai') setHasOpenAiKey(false);
    } catch { /* noop */ }
  };
  const setDraft = (backend: string, value: string) =>
    setDraftKey((s) => ({ ...s, [backend]: value }));
  return { hasKey, draftKey, note, noteRaw, saveKey, clearKey, setDraft };
}
export type ProviderKeys = ReturnType<typeof useProviderKeys>;

/** One API-key row: label, masked input, Save, and Remove once a key is stored.
 *  `plain` is the simple-mode wording — no environment-variable names. */
export function ProviderKeyField({ backend, keys, plain = false }: {
  backend: { id: string; label: string; envVar: string };
  keys: ProviderKeys;
  plain?: boolean;
}) {
  const { t } = useTranslation();
  const b = backend;
  const { hasKey, draftKey, note, noteRaw } = keys;
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <label style={labelStyle}>
        {plain
          ? <>{t('simpleMode.aiConnection.keyFor', { label: b.label })} {hasKey[b.id] ? `· ${t('aiEngines.setCheck')}` : ''}</>
          : <>{b.label} {hasKey[b.id] ? `· ${t('aiEngines.setCheck')}` : ''} <span style={{ opacity: 0.6 }}>({b.envVar})</span></>}
      </label>
      <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
        <input
          type="password"
          autoComplete="off"
          placeholder={hasKey[b.id] ? t('aiEngines.keyStoredPlaceholder') : t('aiEngines.keyPlaceholder', { label: b.label })}
          value={draftKey[b.id] ?? ''}
          onChange={(e) => keys.setDraft(b.id, e.target.value)}
          style={inputStyle}
        />
        <PixelButton variant="secondary" size="sm" onClick={() => keys.saveKey(b.id)} disabled={!(draftKey[b.id] ?? '').trim()}>{t('common.save')}</PixelButton>
        {hasKey[b.id] && (
          <PixelButton variant="secondary" size="sm" onClick={() => keys.clearKey(b.id)}>{t('aiEngines.removeKey')}</PixelButton>
        )}
      </div>
      {note[b.id] && (plain && noteRaw[b.id] !== undefined
        ? <ErrorNotice error={noteRaw[b.id]} legacy={note[b.id]} context="key" friendly layout="inline" />
        : <div style={{ fontSize: 11, color: 'var(--cth-ink-500)' }}>{note[b.id]}</div>)}
    </div>
  );
}

/** Per-CLI local endpoint + default model, blur-saved into HarnessConfig. */
export function useCliEndpoints(config: HarnessConfig) {
  const { t } = useTranslation();
  // Base-URL + default-model drafts, seeded from config.
  const [baseUrls, setBaseUrls] = useState<Partial<Record<AgentProvider, string>>>(
    config.providerBaseUrls ?? {}
  );
  const [models, setModels] = useState<Partial<Record<AgentProvider, string>>>(
    config.providerDefaultModels ?? {}
  );
  // Per-CLI save feedback for the blur-saved base-URL / model fields. Without
  // this the user gets zero signal — the field just keeps its value silently.
  const [cliNote, setCliNote] = useState<Partial<Record<AgentProvider, { ok: boolean; msg: string; raw?: unknown }>>>({});

  const saveBaseUrl = async (id: AgentProvider, value: string) => {
    const next = { ...baseUrls, [id]: value.trim() || undefined };
    setBaseUrls(next);
    try {
      await window.cth.updateConfig({ providerBaseUrls: next });
      setCliNote((s) => ({ ...s, [id]: { ok: true, msg: t('aiEngines.saved') } }));
    } catch (e) {
      setCliNote((s) => ({ ...s, [id]: { ok: false, msg: e instanceof Error ? e.message : String(e), raw: e } }));
    }
  };
  const saveModel = async (id: AgentProvider, value: string) => {
    const next = { ...models, [id]: value.trim() || undefined };
    setModels(next);
    try {
      await window.cth.updateConfig({ providerDefaultModels: next });
      setCliNote((s) => ({ ...s, [id]: { ok: true, msg: t('aiEngines.saved') } }));
    } catch (e) {
      setCliNote((s) => ({ ...s, [id]: { ok: false, msg: e instanceof Error ? e.message : String(e), raw: e } }));
    }
  };
  return { baseUrls, models, cliNote, saveBaseUrl, saveModel };
}
export type CliEndpoints = ReturnType<typeof useCliEndpoints>;

export function AiEnginesSettings({ config }: { config: HarnessConfig }) {
  const { t } = useTranslation();
  const keys = useProviderKeys();
  const { baseUrls, models, cliNote, saveBaseUrl, saveModel } = useCliEndpoints(config);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
      <div>
        <div style={headStyle}>{t('aiEngines.providers')}</div>
        <div style={{ fontSize: 12, color: 'var(--cth-ink-700)', lineHeight: '18px' }}>
          {t('aiEngines.providersDesc')}
        </div>
      </div>

      {/* Backend API keys (write-only) */}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        <div style={headStyle}>{t('aiEngines.apiKeys')}</div>
        <div style={{ fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-500)' }}>{t('aiEngines.keysScope')}</div>
        {BACKENDS.map((b) => (
          <ProviderKeyField key={b.id} backend={b} keys={keys} />
        ))}
      </div>

      {/* Per-CLI local endpoint + default model */}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
        <div style={headStyle}>{t('aiEngines.localEndpoint')}</div>
        {CLIS.map((c) => (
          <div key={c.id} style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <label style={{ ...labelStyle, display: 'flex', alignItems: 'center', gap: 6 }}>
              <ProviderLogo provider={c.id} size={12} /> {c.label}
            </label>
            <div style={{ display: 'flex', gap: 6 }}>
              <input
                placeholder={t('aiEngines.baseUrlPlaceholder')}
                defaultValue={baseUrls[c.id] ?? ''}
                onBlur={(e) => saveBaseUrl(c.id, e.target.value)}
                style={inputStyle}
              />
              <input
                placeholder={t('aiEngines.defaultModelPlaceholder')}
                defaultValue={models[c.id] ?? ''}
                onBlur={(e) => saveModel(c.id, e.target.value)}
                style={{ ...inputStyle, maxWidth: 220 }}
              />
            </div>
            <span style={{ fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-500)' }}>{c.hintKey ? t(c.hintKey) : c.hint}</span>
            {cliNote[c.id] && (
              <span style={{ fontSize: 11, color: cliNote[c.id]!.ok ? 'var(--cth-mint)' : 'var(--cth-coral)' }}>{cliNote[c.id]!.msg}</span>
            )}
          </div>
        ))}
      </div>

      {/* Unsandboxed-in-auto caveat (Pam guardrail #6) */}
      <div style={{
        fontSize: 12, color: 'var(--cth-ink-700)', lineHeight: '17px',
        padding: 8, boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)', background: 'var(--cth-paper-100)'
      }}>
        {t('aiEngines.autoModeCaveat')}
      </div>
    </div>
  );
}
