import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AGENT_PROVIDER_PRESETS,
  providerPreset,
  type AgentProvider,
  type HarnessConfig
} from '@/store/config';
import { engineCostHint } from '@shared/engineCost';
import {
  hermesChoiceFor,
  type HermesSource,
  type HermesUserConfigStatus
} from '@shared/quickStart';
import { HermesSourceChoice } from './HermesSourceChoice';
import { PixelButton } from './PixelButton';
import { ErrorNotice } from './FriendlyError';
import { ProviderLogo } from './ProviderLogo';
import {
  BACKENDS,
  ProviderKeyField,
  inputStyle,
  labelStyle,
  useCliEndpoints,
  useProviderKeys
} from './AiEnginesSettings';

/**
 * Settings → "AI connection" in simple mode: ONE place to see which AI runs the
 * team and to paste (or replace) its key. It is built from the same pieces as the
 * full engines list in advanced mode (provider-key store, base-URL fields), so
 * both write to the same secret broker and the same config keys — just with the
 * wording and the choices cut down to what a first-time user can act on.
 *
 * The engine choice is STAGED by the host (`onPickEngine`) and written by
 * Settings' Save like every other preference; the key is written the moment it is
 * saved (write-only secret broker, never staged).
 */

/** Engines offered up front; everything else sits under "other options". Hermes
 *  first: it is the recommended, low-cost default. */
const PRIMARY: AgentProvider[] = ['hermes', 'claude'];

/** Where a first-time user gets a key for each backend. */
const KEY_PAGES: Record<string, string> = {
  openrouter: 'https://openrouter.ai/keys',
  deepseek: 'https://platform.deepseek.com/api_keys'
};

/** Which key backends make sense for an engine (the first is the default). */
function backendsFor(engine: AgentProvider): string[] {
  if (engine === 'hermes') return ['openrouter', 'deepseek'];
  if (engine === 'claude') return ['anthropic'];
  return BACKENDS.map((b) => b.id);
}

export function SimpleAiConnection({ config, engine, engineChanged, onPickEngine }: {
  config: HarnessConfig;
  /** The orchestrator engine as currently staged in the Settings form. */
  engine: AgentProvider;
  /** True while the staged engine differs from the saved one (not yet applied). */
  engineChanged: boolean;
  onPickEngine: (id: AgentProvider) => void;
}) {
  const { t } = useTranslation();
  const keys = useProviderKeys();
  const endpoints = useCliEndpoints(config);

  // Eligible = what can run the orchestrator (needs an inbox drain) and takes a model.
  const eligible = AGENT_PROVIDER_PRESETS.filter((p) => p.supportsModel && p.canReceiveInbox);
  const primary = PRIMARY.flatMap((id) => eligible.filter((p) => p.id === id));
  const others = eligible.filter((p) => !PRIMARY.includes(p.id));

  const choices = backendsFor(engine);
  // The key the user is looking at: prefer one that is already stored.
  const [picked, setPicked] = useState<string | null>(null);
  const backendId = picked && choices.includes(picked)
    ? picked
    : (choices.find((id) => keys.hasKey[id]) ?? choices[0]);
  const backend = BACKENDS.find((b) => b.id === backendId) ?? BACKENDS[0];
  const stored = choices.some((id) => keys.hasKey[id]);
  const hint = engineCostHint(engine);
  const keyPage = KEY_PAGES[backend.id];

  // The user's OWN Hermes setup (read-only look: provider + whether a key is
  // there). When it has a provider, which setup runs the assistant is an explicit
  // choice, saved as `hermesSource` the moment it is made (like the key — not
  // staged with Save), so the copy of their config can't silently win.
  const [hermesStatus, setHermesStatus] = useState<HermesUserConfigStatus | undefined>();
  useEffect(() => {
    let live = true;
    void Promise.resolve(window.cth.hermesUserConfigStatus?.())
      .then((s) => { if (live) setHermesStatus((s ?? undefined) as HermesUserConfigStatus | undefined); })
      .catch(() => { /* unknown = no choice shown */ });
    return () => { live = false; };
  }, []);
  const choice = hermesChoiceFor(hermesStatus);
  const showChoice = engine === 'hermes' && choice.show;
  // No stored choice reads as 'own' — what the backend does with it.
  const [source, setSource] = useState<HermesSource>(config.hermesSource === 'app' ? 'app' : 'own');
  const [sourceSaved, setSourceSaved] = useState<'idle' | 'saved' | 'failed'>('idle');
  const useOwn = showChoice && source === 'own';
  const pickSource = async (v: HermesSource) => {
    setSource(v);
    setSourceSaved('idle');
    try {
      // The user's own provider must not be handed a model picked for another one.
      await window.cth.updateConfig({ hermesSource: v, ...(v === 'own' ? { godModel: null } : {}) });
      setSourceSaved('saved');
    } catch { setSourceSaved('failed'); }
  };

  const chip = (id: AgentProvider, label: string) => {
    const active = engine === id;
    const cost = engineCostHint(id);
    return (
      <button
        key={id}
        type="button"
        onClick={() => onPickEngine(id)}
        aria-pressed={active}
        style={{
          padding: '6px 12px 5px', border: 'none', cursor: 'pointer',
          background: active ? 'var(--cth-sky-light)' : 'var(--cth-cream-100)',
          boxShadow: active ? 'inset 0 0 0 2px var(--cth-ink-500)' : 'inset 0 0 0 1px var(--cth-ink-100)',
          fontFamily: 'var(--cth-font-ui)', fontSize: 14, color: 'var(--cth-ink-900)',
          display: 'inline-flex', flexDirection: 'column', alignItems: 'flex-start', gap: 2
        }}
      >
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
          <ProviderLogo provider={id} size={16} />
          {label}
        </span>
        {cost && (
          <span style={{ fontSize: 11, color: cost.recommended ? 'var(--cth-ink-900)' : 'var(--cth-ink-500)', fontWeight: cost.recommended ? 700 : 400 }}>
            {t(cost.tagKey)}
          </span>
        )}
      </button>
    );
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }} data-testid="simple-ai-connection">
      <div style={{ fontSize: 13, lineHeight: '20px', color: 'var(--cth-ink-700)' }}>
        {t('simpleMode.aiConnection.intro')}
      </div>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        <span style={labelStyle}>{t('simpleMode.aiConnection.whichAi')}</span>
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'stretch' }}>
          {primary.map((p) => chip(p.id, p.label))}
          {others.length > 0 && (
            <select
              value={PRIMARY.includes(engine) ? '' : engine}
              onChange={(e) => { if (e.target.value) onPickEngine(e.target.value as AgentProvider); }}
              aria-label={t('simpleMode.aiConnection.otherOptions')}
              style={{ ...inputStyle, width: 'auto', fontSize: 13 }}
            >
              <option value="">{t('simpleMode.aiConnection.otherOptions')}</option>
              {others.map((p) => <option key={p.id} value={p.id}>{p.label}</option>)}
            </select>
          )}
        </div>
        {hint && (
          <span style={{ fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-700)' }}>{t(hint.descKey)}</span>
        )}
        {engineChanged && (
          <span style={{ fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-500)' }}>
            {t('simpleMode.aiConnection.appliesOnRestart', { engine: providerPreset(engine).label })}
          </span>
        )}
      </div>

      <div style={{ height: 1, background: 'var(--cth-ink-300)' }} />

      {showChoice && choice.provider && (
        <>
          <HermesSourceChoice
            name="aiConnectionHermesSource"
            provider={choice.provider}
            hasKeys={choice.hasKeys}
            value={source}
            onChange={(v) => { void pickSource(v); }}
          />
          {sourceSaved === 'saved' && (
            <span role="status" style={{ fontSize: 12, color: 'var(--cth-ink-700)' }}>{t('firstRun.choice.saved')}</span>
          )}
          {sourceSaved === 'failed' && (
            <span role="alert" style={{ fontSize: 12, color: 'var(--cth-ink-900)' }}>{t('firstRun.choice.saveFailed')}</span>
          )}
          <div style={{ height: 1, background: 'var(--cth-ink-300)' }} />
        </>
      )}

      {useOwn ? (
        <span style={{ fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-700)' }}>
          {t('firstRun.choice.ownChosen')}
        </span>
      ) : (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        <span style={labelStyle}>{t('simpleMode.aiConnection.yourKey')}</span>
        <span style={{ fontSize: 12, lineHeight: '17px', color: stored ? 'var(--cth-ink-900)' : 'var(--cth-ink-700)' }}>
          {stored ? t('simpleMode.aiConnection.keySaved') : t('simpleMode.aiConnection.noKey')}
        </span>
        {choices.length > 1 && (
          <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span style={labelStyle}>{t('simpleMode.aiConnection.keyFrom')}</span>
            <select
              value={backend.id}
              onChange={(e) => setPicked(e.target.value)}
              style={{ ...inputStyle, width: 'auto', minWidth: 200 }}
            >
              {choices.map((id) => {
                const b = BACKENDS.find((x) => x.id === id);
                return b ? <option key={id} value={id}>{b.label}{keys.hasKey[id] ? ` · ${t('aiEngines.setCheck')}` : ''}</option> : null;
              })}
            </select>
          </label>
        )}
        <ProviderKeyField backend={backend} keys={keys} plain />
        {keyPage && (
          <div>
            <PixelButton variant="ghost" size="sm" onClick={() => void window.cth.openExternal(keyPage)}>
              {t('simpleMode.aiConnection.getKey', { label: backend.label })}
            </PixelButton>
          </div>
        )}
        {engine === 'claude' && (
          <span style={{ fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-500)' }}>
            {t('simpleMode.aiConnection.claudeLogin')}
          </span>
        )}
        {engine === 'hermes' && backend.id === 'deepseek' && (
          <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
            <span style={labelStyle}>{t('simpleMode.aiConnection.serverAddress')}</span>
            <input
              placeholder="https://api.deepseek.com"
              defaultValue={endpoints.baseUrls.hermes ?? ''}
              onBlur={(e) => endpoints.saveBaseUrl('hermes', e.target.value)}
              style={inputStyle}
            />
            <span style={{ fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-500)' }}>
              {t('simpleMode.aiConnection.serverAddressHint')}
            </span>
            {endpoints.cliNote.hermes && (endpoints.cliNote.hermes.ok || endpoints.cliNote.hermes.raw === undefined ? (
              <span style={{ fontSize: 11, color: endpoints.cliNote.hermes.ok ? 'var(--cth-mint)' : 'var(--cth-coral)' }}>
                {endpoints.cliNote.hermes.msg}
              </span>
            ) : (
              <ErrorNotice error={endpoints.cliNote.hermes.raw} legacy={endpoints.cliNote.hermes.msg} context="save" friendly layout="inline" />
            ))}
          </label>
        )}
        <span style={{ fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-500)' }}>
          {t('simpleMode.aiConnection.keyPrivacy')}
        </span>
      </div>
      )}
    </div>
  );
}
