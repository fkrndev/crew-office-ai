import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { HarnessConfig } from '@/store/config';
import { MCP_CATALOG, type McpTier } from '@shared/mcpCatalog';

export interface McpDefaultsSettingsProps {
  config: HarnessConfig;
}

const TIER_ORDER: McpTier[] = ['safe-readonly', 'write', 'secret'];
const TIER_LABEL_KEY: Record<McpTier, string> = {
  'safe-readonly': 'mcpDefaults.tiers.safeReadonly',
  'write': 'mcpDefaults.tiers.write',
  'secret': 'mcpDefaults.tiers.secret'
};
const TIER_NOTE_KEY: Record<McpTier, string> = {
  'safe-readonly': 'mcpDefaults.tiers.safeReadonlyNote',
  'write': 'mcpDefaults.tiers.writeNote',
  'secret': 'mcpDefaults.tiers.secretNote'
};

const labelStyle: React.CSSProperties = {
  fontFamily: 'var(--cth-font-display)',
  fontSize: 8,
  lineHeight: '12px',
  color: 'var(--cth-ink-500)',
  textTransform: 'uppercase'
};

export function McpDefaultsSettings({ config }: McpDefaultsSettingsProps) {
  const { t } = useTranslation();
  const [note, setNote] = useState('');
  // Consent switches are LIVE — they persist on click like the Slack
  // integrations beside them, not via the modal's staged Save. The config
  // prop doesn't reflow after a write, so the switch reads an optimistic
  // local override — without it the toggle visibly snapped back and looked
  // like a dead button.
  const [overrides, setOverrides] = useState<Record<string, boolean>>({});

  const enabledFor = (id: string): boolean =>
    overrides[id]
      ?? config.mcpDefaults?.[id]?.enabled
      ?? MCP_CATALOG.find((e) => e.id === id)?.defaultEnabled
      ?? false;

  const toggle = async (id: string) => {
    const next = !enabledFor(id);
    // Flip first — the switch is the user's answer, not a save probe.
    setOverrides((prev) => ({ ...prev, [id]: next }));
    try {
      await window.cth.updateConfig({
        mcpDefaults: {
          ...(config.mcpDefaults ?? {}),
          // Fold earlier optimistic flips in — the prop still shows pre-toggle
          // values, so a second toggle would otherwise drop the first write.
          ...Object.fromEntries(
            Object.entries(overrides).map(([k, v]) => [k, { enabled: v }])
          ),
          [id]: { enabled: next }
        }
      });
      setNote(t('mcpDefaults.toggleNote', { id, state: next ? t('common.on') : t('common.off') }));
      setTimeout(() => setNote(''), 1800);
    } catch {
      // Roll the optimistic flip back — the switch must not lie about consent.
      setOverrides((prev) => ({ ...prev, [id]: !next }));
      setNote(t('mcpDefaults.couldNotSave'));
      setTimeout(() => setNote(''), 2000);
    }
  };

  const byTier = (tier: McpTier) => MCP_CATALOG.filter((e) => e.tier === tier);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 16 }}>
      <div>
        <div style={{ ...labelStyle, marginBottom: 6 }}>{t('mcpDefaults.title')}</div>
        <span style={{ fontSize: 12, lineHeight: '16px', color: 'var(--cth-ink-500)' }}>
          {t('mcpDefaults.desc')}
        </span>
      </div>

      {TIER_ORDER.map((tier) => {
        const entries = byTier(tier);
        if (entries.length === 0) return null;
        const isConsent = tier !== 'safe-readonly';
        return (
          <div key={tier} style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
              <span style={{
                fontFamily: 'var(--cth-font-display)', fontSize: 8, lineHeight: '12px',
                color: isConsent ? '#6E1423' : 'var(--cth-ink-500)',
                textTransform: 'uppercase'
              }}>
                {t(TIER_LABEL_KEY[tier])}
              </span>
              <span style={{ fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-400, var(--cth-ink-500))' }}>
                {t(TIER_NOTE_KEY[tier])}
              </span>
            </div>

            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {entries.map((entry) => {
                const on = enabledFor(entry.id);
                return (
                  <div
                    key={entry.id}
                    style={{
                      display: 'flex', alignItems: 'center', justifyContent: 'space-between',
                      gap: 12, padding: '7px 10px',
                      background: 'var(--cth-paper-100)',
                      boxShadow: `inset 0 0 0 1px ${isConsent && on ? '#6E1423' : 'var(--cth-ink-300)'}`
                    }}
                  >
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 1, flex: 1, minWidth: 0 }}>
                      <span style={{ fontSize: 12, lineHeight: '18px', color: 'var(--cth-ink-900)', fontWeight: 600 }}>
                        {entry.label}
                        <code style={{
                          marginLeft: 6,
                          fontFamily: 'var(--cth-font-mono)',
                          fontSize: 11,
                          color: 'var(--cth-ink-500)',
                          fontWeight: 400
                        }}>{entry.id}</code>
                      </span>
                      <span style={{ fontSize: 12, lineHeight: '16px', color: 'var(--cth-ink-500)', wordBreak: 'break-word' }}>
                        {entry.description}
                      </span>
                    </div>
                    <button
                      type="button"
                      onClick={() => { void toggle(entry.id); }}
                      style={{
                        flexShrink: 0,
                        padding: '3px 10px 1px',
                        background: on
                          ? (isConsent ? 'var(--cth-coral-light, #f6d3c4)' : 'var(--cth-lemon)')
                          : 'var(--cth-cream-200)',
                        boxShadow: `inset 0 0 0 1px ${on ? 'var(--cth-ink-900)' : 'var(--cth-ink-700)'}`,
                        border: 'none',
                        fontFamily: 'var(--cth-font-display)',
                        fontSize: 8,
                        lineHeight: '14px',
                        color: 'var(--cth-ink-900)',
                        cursor: 'pointer',
                        textTransform: 'uppercase'
                      }}
                    >
                      {on ? t('common.on') : t('common.off')}
                    </button>
                  </div>
                );
              })}
            </div>
          </div>
        );
      })}

      <ShardXSetup />

      {note && (
        <span style={{ fontSize: 12, color: 'var(--cth-mint)' }}>{note}</span>
      )}
    </div>
  );
}

interface ShardXStatus {
  launcherRunning?: boolean;
  mcpInstalled?: boolean;
  tokenSet?: boolean;
  api?: string;
  mcpDir?: string;
}

interface ShardXProfileInfo {
  id?: string;
  name?: string;
  folder?: string;
  running?: boolean;
  owner?: string;
  auto?: boolean;
}

const shardxBtnStyle: React.CSSProperties = {
  flexShrink: 0,
  padding: '3px 10px 1px',
  background: 'var(--cth-cream-200)',
  boxShadow: 'inset 0 0 0 1px var(--cth-ink-700)',
  border: 'none',
  fontFamily: 'var(--cth-font-display)',
  fontSize: 8,
  lineHeight: '14px',
  color: 'var(--cth-ink-900)',
  cursor: 'pointer',
  textTransform: 'uppercase'
};

/** The 'shardx' catalog entry needs three local gates the toggle can't provide:
 *  the launcher app running, the MCP bridge vendored under the harness, and an
 *  Automation API token in the keychain. Without this card those cth.shardx*
 *  commands were unreachable — the toggle armed a dead server path. */
function ShardXSetup() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<ShardXStatus | null>(null);
  const [profiles, setProfiles] = useState<ShardXProfileInfo[] | null>(null);
  const [token, setToken] = useState('');
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus((await window.cth.shardxStatus()) as ShardXStatus);
    } catch {
      /* backend unreachable — keep the last reading */
    }
    try {
      const r = (await window.cth.shardxProfiles()) as
        | { ok?: boolean; profiles?: ShardXProfileInfo[] }
        | undefined;
      setProfiles(r?.ok ? (r.profiles ?? []) : null);
    } catch {
      setProfiles(null);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const flash = (msg: string) => {
    setNote(msg);
    setTimeout(() => setNote(''), 3200);
  };

  const run = async (fn: () => Promise<unknown>) => {
    if (busy) return;
    setBusy(true);
    // Bootstrap can run minutes (launcher download + npm install). Without a
    // working note the whole card just looks dead-disabled.
    setNote(t('mcpDefaults.shardx.working'));
    try {
      const r = (await fn()) as
        | { ok?: boolean; path?: string; error?: string; removed?: number; kept?: number }
        | undefined;
      if (r && typeof r === 'object' && 'ok' in r) {
        flash(
          r.ok
            ? r.removed !== undefined
              ? t('mcpDefaults.shardx.cleaned', { removed: r.removed, kept: r.kept ?? 0 })
              : r.path
                ? t('mcpDefaults.shardx.savedTo', { path: r.path })
                : t('mcpDefaults.shardx.done')
            : (() => {
                const steps = (r as { steps?: Record<string, { ok?: boolean }> }).steps;
                const bad = steps
                  ? Object.entries(steps)
                      .filter(([, v]) => v && v.ok === false)
                      .map(([k]) => k)
                      .join(', ')
                  : '';
                const base = r.error ?? t('mcpDefaults.shardx.failed');
                return bad ? `${base}: ${bad}` : base;
              })()
        );
      } else {
        flash(t('mcpDefaults.shardx.done'));
      }
    } catch {
      flash(t('mcpDefaults.shardx.failed'));
    } finally {
      setBusy(false);
      void refresh();
    }
  };

  const saveToken = async () => {
    const v = token.trim();
    if (!v || busy) return;
    setBusy(true);
    try {
      await window.cth.shardxSetToken(v);
      setToken('');
      flash(t('mcpDefaults.shardx.done'));
    } catch {
      flash(t('mcpDefaults.shardx.failed'));
    } finally {
      setBusy(false);
      void refresh();
    }
  };

  const gates: Array<[string, boolean]> = [
    [t('mcpDefaults.shardx.gateLauncher'), !!status?.launcherRunning],
    [t('mcpDefaults.shardx.gateBridge'), !!status?.mcpInstalled],
    [t('mcpDefaults.shardx.gateToken'), !!status?.tokenSet]
  ];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
        <span style={{ ...labelStyle, color: '#6E1423' }}>{t('mcpDefaults.shardx.title')}</span>
        <span style={{ fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-400, var(--cth-ink-500))' }}>
          {t('mcpDefaults.shardx.desc')}
        </span>
      </div>

      <div
        style={{
          display: 'flex', flexDirection: 'column', gap: 8, padding: '8px 10px',
          background: 'var(--cth-paper-100)',
          boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)'
        }}
      >
        <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap' }}>
          {gates.map(([label, ok]) => (
            <span key={label} style={{ display: 'flex', alignItems: 'center', gap: 5 }}>
              <span
                style={{
                  width: 7, height: 7,
                  background: ok ? 'var(--cth-mint)' : 'var(--cth-cream-200)',
                  boxShadow: `inset 0 0 0 1px ${ok ? 'var(--cth-ink-900)' : 'var(--cth-ink-500)'}`
                }}
              />
              <span style={{ fontSize: 11, lineHeight: '14px', color: 'var(--cth-ink-700)' }}>{label}</span>
            </span>
          ))}
        </div>

        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
          <button
            type="button"
            disabled={busy}
            onClick={() => void run(() => window.cth.shardxBootstrap())}
            style={{
              ...shardxBtnStyle,
              background: 'var(--cth-lemon)',
              boxShadow: 'inset 0 0 0 1px var(--cth-ink-900)'
            }}
          >
            {t('mcpDefaults.shardx.autoSetup')}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void run(() => window.cth.shardxDownloadLauncher())}
            style={shardxBtnStyle}
          >
            {t('mcpDefaults.shardx.downloadLauncher')}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void run(() => window.cth.shardxSetup())}
            style={shardxBtnStyle}
          >
            {t('mcpDefaults.shardx.installBridge')}
          </button>
          <button
            type="button"
            disabled={busy}
            onClick={() => void refresh()}
            style={shardxBtnStyle}
          >
            {t('mcpDefaults.shardx.refresh')}
          </button>
          {/* Orphan sweep — deletes ONLY auto-created office-* profiles no
              registry entry still owns; user-named profiles never touched.
              Destructive → explicit confirm. */}
          <button
            type="button"
            disabled={busy || !status?.launcherRunning || !status?.tokenSet}
            onClick={() => {
              if (!window.confirm(t('mcpDefaults.shardx.cleanConfirm'))) return;
              void run(() => window.cth.shardxCleanup());
            }}
            style={shardxBtnStyle}
          >
            {t('mcpDefaults.shardx.cleanProfiles')}
          </button>
        </div>

        {/* Profile inventory — name, live state, owning agent. `office-*` =
            auto-created (cleanup candidates); anything else is user-named
            and never auto-deleted. */}
        {profiles !== null && (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
            {profiles.length === 0 && (
              <span style={{ fontSize: 11, color: 'var(--cth-ink-500)' }}>
                {t('mcpDefaults.shardx.noProfiles')}
              </span>
            )}
            {profiles.map((p) => (
              <span key={p.id ?? p.name} style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 11 }}>
                <span
                  style={{
                    width: 7, height: 7, flexShrink: 0,
                    background: p.running ? 'var(--cth-mint)' : 'var(--cth-cream-200)',
                    boxShadow: `inset 0 0 0 1px ${p.running ? 'var(--cth-ink-900)' : 'var(--cth-ink-500)'}`
                  }}
                />
                <span style={{ fontFamily: 'var(--cth-font-mono)', color: 'var(--cth-ink-900)' }}>
                  {p.name}
                </span>
                {p.auto && (
                  <span style={{ fontSize: 10, color: 'var(--cth-ink-400, var(--cth-ink-500))' }}>auto</span>
                )}
                <span style={{ flex: 1 }} />
                {p.owner && (
                  <span style={{ fontSize: 10, color: 'var(--cth-ink-500)' }}>
                    {t('mcpDefaults.shardx.ownedBy', { agent: p.owner })}
                  </span>
                )}
              </span>
            ))}
          </div>
        )}

        <div style={{ display: 'flex', gap: 6 }}>
          <input
            type="password"
            value={token}
            placeholder={t('mcpDefaults.shardx.tokenPlaceholder')}
            onChange={(e) => setToken(e.target.value)}
            style={{
              flex: 1, minWidth: 0, padding: '4px 8px',
              background: 'var(--cth-cream-100, var(--cth-paper-100))',
              border: 'none',
              boxShadow: 'inset 0 0 0 1px var(--cth-ink-500)',
              fontFamily: 'var(--cth-font-mono)',
              fontSize: 11,
              color: 'var(--cth-ink-900)'
            }}
          />
          <button
            type="button"
            disabled={busy || !token.trim()}
            onClick={() => void saveToken()}
            style={shardxBtnStyle}
          >
            {t('mcpDefaults.shardx.saveToken')}
          </button>
        </div>

        {note && (
          <span style={{ fontSize: 11, lineHeight: '14px', color: 'var(--cth-ink-700)', wordBreak: 'break-all' }}>
            {note}
          </span>
        )}
      </div>
    </div>
  );
}
