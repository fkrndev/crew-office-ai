import { useTranslation } from 'react-i18next';
import type { HermesSource } from '@shared/quickStart';

/**
 * "Which Hermes setup should your assistant use?" — shown only when the user
 * ALREADY has a Hermes config with a provider of its own. Without the choice the
 * per-agent copy of their config silently won over whatever they picked here.
 *
 *  - own: their existing setup, no key needed in this app (recommended when it
 *    already holds a key);
 *  - app: the provider / key set in this app replace theirs for the assistant —
 *    their own Hermes files are never modified.
 *
 * Controlled; the host persists the choice (`hermesSource` in config).
 */
export function HermesSourceChoice({
  provider, hasKeys, value, onChange, name
}: {
  provider: string;
  hasKeys: boolean;
  value: HermesSource;
  onChange: (v: HermesSource) => void;
  /** Radio group name (unique per host). */
  name: string;
}) {
  const { t } = useTranslation();
  const row = (id: HermesSource, label: string, note: string, badge?: string) => {
    const sel = value === id;
    return (
      <label
        key={id}
        style={{
          display: 'flex', alignItems: 'flex-start', gap: 10, padding: '8px 10px', cursor: 'pointer',
          background: sel ? 'var(--cth-mint-light)' : 'var(--cth-paper-100)',
          boxShadow: `inset 0 0 0 ${sel ? 2 : 1}px ${sel ? 'var(--cth-mint)' : 'var(--cth-ink-300)'}`
        }}
      >
        <input
          type="radio"
          name={name}
          value={id}
          checked={sel}
          onChange={() => onChange(id)}
          style={{ width: 16, height: 16, flexShrink: 0, marginTop: 2 }}
        />
        <span style={{ flex: 1, minWidth: 0 }}>
          <span style={{ display: 'block', fontSize: 13, lineHeight: '18px', color: 'var(--cth-ink-900)' }}>{label}</span>
          <span style={{ display: 'block', fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-500)' }}>{note}</span>
        </span>
        {badge && (
          <span style={{
            fontSize: 10, padding: '1px 5px', lineHeight: '16px', flexShrink: 0,
            background: 'var(--cth-lemon)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
            fontFamily: 'var(--cth-font-display)'
          }}>{badge}</span>
        )}
      </label>
    );
  };
  return (
    <div role="radiogroup" aria-label={t('firstRun.choice.legend')} data-testid="hermes-source-choice"
      style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
      <div style={{ fontFamily: 'var(--cth-font-display)', fontSize: 10, color: 'var(--cth-ink-700)' }}>
        {t('firstRun.choice.legend')}
      </div>
      <div style={{ fontSize: 12, lineHeight: '17px', color: 'var(--cth-ink-700)' }}>
        {t('firstRun.choice.intro')}
      </div>
      {row(
        'own',
        t('firstRun.choice.own', { provider }),
        hasKeys ? t('firstRun.choice.ownNote') : t('firstRun.choice.ownNoKeyNote'),
        hasKeys ? t('firstRun.choice.recommended') : undefined
      )}
      {row('app', t('firstRun.choice.app'), t('firstRun.choice.appNote'))}
    </div>
  );
}
