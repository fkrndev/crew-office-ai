import { useTranslation } from 'react-i18next';
import { providerPreset, type AgentProvider } from '@shared/agentProvider';
import { providerNoteKeys } from '@shared/providerCapabilities';

/**
 * Short, honest per-engine notes for the provider pickers: what will NOT work for
 * the selected engine (cost meter, skills…) and what to do on its first run.
 * Data-driven from shared/providerCapabilities — renders nothing for engines
 * (Claude) that have nothing to warn about.
 */
export function ProviderNotes({ provider, autoMode }: { provider: AgentProvider; autoMode: boolean }) {
  const { t } = useTranslation();
  const keys = providerNoteKeys(provider, { autoMode });
  if (keys.length === 0) return null;
  const label = providerPreset(provider).label;
  return (
    <div
      data-testid="provider-notes"
      style={{
        display: 'flex', flexDirection: 'column', gap: 4, padding: '6px 8px',
        background: 'var(--cth-lemon-light)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
        fontSize: 12, lineHeight: '16px', color: 'var(--cth-ink-700)'
      }}
    >
      {keys.map((k) => <span key={k}>{t(k, { provider: label })}</span>)}
    </div>
  );
}
