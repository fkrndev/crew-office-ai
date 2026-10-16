import { useTranslation } from 'react-i18next';
import { PixelButton } from './PixelButton';
import { Icon } from './Icon';
import { HermesSourceChoice } from './HermesSourceChoice';
import {
  QUICK_START_PROVIDERS,
  quickStartKeyLooksOff,
  quickStartProvider,
  validateQuickStartKey,
  type HermesChoice,
  type HermesSource,
  type QuickStartProviderId
} from '@shared/quickStart';

/** Where the optional key check stands. 'rejected' / 'unreachable' keep the user on
 *  the step ONCE (so they can fix a typo) — pressing Next again continues. */
export type QuickStartCheckStatus = 'idle' | 'checking' | 'ok' | 'rejected' | 'unreachable';

export interface QuickStartProps {
  provider: QuickStartProviderId;
  onProvider: (id: QuickStartProviderId) => void;
  apiKey: string;
  onApiKey: (key: string) => void;
  /** The model as shown in the Advanced field (the provider default until edited). */
  model: string;
  onModel: (model: string) => void;
  check: QuickStartCheckStatus;
  /** Hermes is already on this machine (it may have its own provider configured). */
  hermesInstalled?: boolean;
  /** "I already use Claude / Codex / another CLI" — opens the normal engine step. */
  onUseCli: () => void;
  /** The user's own Hermes config, when it has a provider: shows the explicit
   *  "use my existing setup / use the key I paste here" choice (see
   *  HermesSourceChoice). Absent / `show:false` = today's step, unchanged. */
  hermesChoice?: HermesChoice;
  hermesSource?: HermesSource;
  onHermesSource?: (s: HermesSource) => void;
}

/** The ONE friendly onboarding step for non-technical people (or anyone who picks
 *  "quick start"): what an API key is, where to get one, a paste field, an honest
 *  cost note — instead of the engine / model / orchestrator steps. Controlled: the
 *  wizard owns the state and does the check + the save (see shared/quickStart.ts). */
export function QuickStart({
  provider, onProvider, apiKey, onApiKey, model, onModel, check, hermesInstalled, onUseCli,
  hermesChoice, hermesSource, onHermesSource
}: QuickStartProps) {
  const { t } = useTranslation();
  const chosen = quickStartProvider(provider);
  const format = validateQuickStartKey(apiKey);
  // Typing feedback only once there is something to judge; an empty field is a
  // legitimate "skip", not an error.
  const formatError =
    !format.ok && format.reason === 'short' ? t('quickStart.errShort')
    : !format.ok && format.reason === 'whitespace' ? t('quickStart.errWhitespace')
    : undefined;
  const prefixHint = format.ok && quickStartKeyLooksOff(provider, apiKey)
    ? t('quickStart.hintPrefix', { provider: chosen.label, prefix: chosen.keyPrefix })
    : undefined;
  const checkMsg =
    check === 'checking' ? t('quickStart.checking')
    : check === 'ok' ? t('quickStart.checkOk')
    : check === 'rejected' ? t('quickStart.checkRejected', { provider: chosen.label })
    : check === 'unreachable' ? t('quickStart.checkUnreachable', { provider: chosen.label })
    : undefined;

  // The user already has a Hermes of their own with a provider: ask which setup
  // runs the assistant. "Own" needs nothing from this step, so the rest is hidden.
  const askSource = hermesChoice?.show === true;
  const source: HermesSource = hermesSource ?? hermesChoice?.defaultSource ?? 'app';
  const useOwn = askSource && source === 'own';
  // ONE "RECOMMENDED" on screen: when the choice above already carries it (their own
  // setup, which holds a key), the provider list does not repeat it — with "paste a
  // key" selected there used to be two.
  const ownIsRecommended = askSource && hermesChoice?.hasKeys === true;

  return (
    <>
      <p style={{ margin: 0, lineHeight: '22px' }}>{t('quickStart.intro')}</p>

      {askSource && hermesChoice?.provider && (
        <HermesSourceChoice
          name="quickStartHermesSource"
          provider={hermesChoice.provider}
          hasKeys={hermesChoice.hasKeys}
          value={source}
          onChange={(v) => onHermesSource?.(v)}
        />
      )}

      {useOwn && (
        <div role="status" style={{
          fontSize: 12, lineHeight: '17px', padding: '6px 8px', color: 'var(--cth-ink-900)',
          background: 'var(--cth-mint-light)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)'
        }}>{t('firstRun.choice.ownChosen')}</div>
      )}

      {!useOwn && (<>
      {/* What is an API key — plain words, no jargon. */}
      <div style={{
        display: 'flex', gap: 8, alignItems: 'flex-start', padding: 10,
        background: 'var(--cth-lemon-light)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
        fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-700)'
      }}>
        <span style={{ flexShrink: 0, marginTop: 1 }}><Icon name="sparkle" /></span>
        <span>
          <strong>{t('quickStart.whatIsKeyHead')}</strong> {t('quickStart.whatIsKey')}
        </span>
      </div>

      <div style={{ fontFamily: 'var(--cth-font-display)', fontSize: 10, color: 'var(--cth-ink-700)' }}>
        {t('quickStart.chooseHead')}
      </div>
      <div role="radiogroup" aria-label={t('quickStart.chooseHead')} style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        {QUICK_START_PROVIDERS.map((p) => {
          const sel = p.id === provider;
          return (
            <div key={p.id} style={{
              display: 'flex', alignItems: 'center', gap: 10, padding: '8px 10px',
              background: sel ? 'var(--cth-mint-light)' : 'var(--cth-paper-100)',
              boxShadow: `inset 0 0 0 ${sel ? 2 : 1}px ${sel ? 'var(--cth-mint)' : 'var(--cth-ink-300)'}`
            }}>
              <label style={{ display: 'flex', alignItems: 'center', gap: 10, flex: 1, minWidth: 0, cursor: 'pointer' }}>
                <input
                  type="radio"
                  name="quickStartProvider"
                  value={p.id}
                  checked={sel}
                  onChange={() => onProvider(p.id)}
                  style={{ width: 16, height: 16, flexShrink: 0 }}
                />
                <span style={{ flex: 1, minWidth: 0 }}>
                  <span style={{ display: 'block', fontFamily: 'var(--cth-font-display)', fontSize: 11 }}>
                    {p.label.toUpperCase()}
                  </span>
                  <span style={{ display: 'block', fontSize: 11, color: 'var(--cth-ink-500)' }}>
                    {t(`quickStart.${p.id}Blurb`)}
                  </span>
                </span>
                {p.recommended && !ownIsRecommended && (
                  <span style={{
                    fontSize: 10, padding: '1px 5px', lineHeight: '16px',
                    background: 'var(--cth-lemon)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
                    fontFamily: 'var(--cth-font-display)', flexShrink: 0
                  }}>{t('quickStart.recommended')}</span>
                )}
              </label>
              <PixelButton variant="secondary" size="sm" onClick={() => { void window.cth.openExternal(p.keyUrl); }}>
                <span style={{ display: 'inline-flex', gap: 4, alignItems: 'center' }}>
                  <Icon name="arrow-right" /> {t('quickStart.openKeyPage')}
                </span>
              </PixelButton>
            </div>
          );
        })}
      </div>
      <div style={{ fontSize: 12, color: 'var(--cth-ink-500)' }}>{t('quickStart.stepsHint')}</div>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
        <label htmlFor="quickStartKey" style={{ fontFamily: 'var(--cth-font-display)', fontSize: 10, color: 'var(--cth-ink-700)' }}>
          {t('quickStart.keyLabel')}
        </label>
        <input
          id="quickStartKey"
          type="password"
          autoComplete="off"
          spellCheck={false}
          value={apiKey}
          onChange={(e) => onApiKey(e.target.value)}
          placeholder={t('quickStart.keyPlaceholder')}
          style={inputStyle}
        />
        {formatError && <div role="alert" style={{ fontSize: 12, color: 'var(--cth-ink-900)' }}>{formatError}</div>}
        {!formatError && prefixHint && <div style={{ fontSize: 12, color: 'var(--cth-ink-700)' }}>{prefixHint}</div>}
        {checkMsg && (
          <div role="status" style={{
            fontSize: 12, padding: '4px 8px', color: 'var(--cth-ink-900)',
            background: check === 'ok' ? 'var(--cth-mint-light)' : check === 'checking' ? 'var(--cth-paper-100)' : 'var(--cth-lemon-light)',
            boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)'
          }}>{checkMsg}</div>
        )}
        <div style={{ fontSize: 12, color: 'var(--cth-ink-500)' }}>{t('quickStart.skipNote')}</div>
      </div>

      <div style={{ fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-700)' }}>
        {t('quickStart.costNote')}
      </div>
      <div style={{ fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-500)' }}>
        {t('quickStart.installNote')}
        {hermesInstalled && <> {t('quickStart.hermesInstalledNote')}</>}
      </div>

      {/* Advanced: the model is editable, but nobody has to look at it. */}
      <details>
        <summary style={{ cursor: 'pointer', fontSize: 12, color: 'var(--cth-ink-700)' }}>
          {t('quickStart.advanced')}
        </summary>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 4, marginTop: 8 }}>
          <label htmlFor="quickStartModel" style={{ fontSize: 12, color: 'var(--cth-ink-500)' }}>
            {t('quickStart.modelLabel')}
          </label>
          <input
            id="quickStartModel"
            value={model}
            onChange={(e) => onModel(e.target.value)}
            spellCheck={false}
            style={inputStyle}
          />
          <div style={{ fontSize: 12, color: 'var(--cth-ink-500)' }}>{t('quickStart.modelNote')}</div>
        </div>
      </details>
      </>)}

      {/* The collapsed way out: a CLI the user already has goes through the normal
          engine step. A link, not a button, so it never competes with Next. */}
      <button
        type="button"
        onClick={onUseCli}
        style={{
          alignSelf: 'flex-start', background: 'none', border: 'none', padding: 0,
          cursor: 'pointer', fontSize: 12, color: 'var(--cth-ink-700)', textDecoration: 'underline'
        }}
      >
        {t('quickStart.useCli')}
      </button>
    </>
  );
}

const inputStyle: React.CSSProperties = {
  width: '100%',
  boxSizing: 'border-box',
  padding: '6px 8px 4px',
  background: 'var(--cth-paper-100)',
  border: 'none',
  boxShadow: 'inset 0 0 0 1px var(--cth-ink-100)',
  fontFamily: 'var(--cth-font-mono)',
  fontSize: 13,
  color: 'var(--cth-ink-900)',
  outline: 'none'
};
