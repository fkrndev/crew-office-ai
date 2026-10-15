import { useEffect, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { PixelPanel } from './PixelPanel';
import { PixelButton } from './PixelButton';
import { SpritePortrait } from './SpritePortrait';
import { ProviderLogo } from './ProviderLogo';
import { ProviderNotes } from './ProviderNotes';
import { capabilitiesForProvider } from '@shared/providerCapabilities';
import { useStore, type Agent } from '@/store/store';
import { OFFICE_CAST, type OfficeCharacterName } from '@/scene/office/cast';
import { useEscape } from '@/hooks/useEscape';
import { useIsSimpleMode } from '@/hooks/useUiMode';
import { engineCostHint } from '@shared/engineCost';
import { type AccentColorName } from '@/design/tokens';
import {
  type AgentProvider,
  type HarnessConfig,
  AGENT_PROVIDER_PRESETS,
  buildSpawnCommand,
  modelsForProvider,
  inferAgentProvider,
  providerPreset,
  isClaudeProvider
} from '@/store/config';

const ACCENTS: AccentColorName[] = ['coral', 'mint', 'sky', 'lemon', 'lilac', 'peach'];

export interface EditAgentModalProps {
  agent: Agent;
  onClose: () => void;
}

/**
 * Compact post-hire editor for Identity / Engine / Briefing. Mirrors the Add
 * Agent fields that matter after spawn; save only patches the durable roster
 * via updateAgent (engine changes apply on the next restart).
 */
export function EditAgentModal({ agent, onClose }: EditAgentModalProps) {
  const updateAgent = useStore((s) => s.updateAgent);
  const { t } = useTranslation();
  // Simple mode: name + "what should this teammate do?" on one short form, the
  // rest (look, engine, one-line role) behind "More options".
  const simple = useIsSimpleMode();
  const sctx = simple ? { context: 'simple' } : undefined;
  const [moreOpen, setMoreOpen] = useState(false);
  const [config, setConfig] = useState<HarnessConfig | null>(null);

  const [name, setName] = useState(agent.name);
  const [character, setCharacter] = useState<OfficeCharacterName>(agent.character);
  const [accent, setAccent] = useState<AccentColorName>(agent.accent);
  const [provider, setProvider] = useState<AgentProvider>(
    inferAgentProvider(agent.command, agent.provider)
  );
  const [model, setModel] = useState<string | undefined>(agent.model);
  const [description, setDescription] = useState(agent.description);
  const [goal, setGoal] = useState(agent.goal ?? '');

  useEffect(() => {
    void window.cth.getConfig().then(setConfig).catch(() => setConfig(null));
  }, []);

  // Keep form in sync when the selected agent changes while the modal is open.
  useEffect(() => {
    setName(agent.name);
    setCharacter(agent.character);
    setAccent(agent.accent);
    setProvider(inferAgentProvider(agent.command, agent.provider));
    setModel(agent.model);
    setDescription(agent.description);
    setGoal(agent.goal ?? '');
  }, [agent.id]);

  const pickProvider = (id: AgentProvider) => {
    setProvider(id);
    if (!config) {
      setModel(undefined);
      return;
    }
    const nextModel = isClaudeProvider(id) ? config.defaultModel : config.providerDefaultModels?.[id];
    setModel(nextModel);
  };

  const preset = providerPreset(provider);

  // Dirty check — any field that differs from the agent's current values.
  // Without this, clicking the backdrop or pressing Esc silently drops edits.
  const dirty =
    name !== agent.name ||
    character !== agent.character ||
    accent !== agent.accent ||
    provider !== inferAgentProvider(agent.command, agent.provider) ||
    model !== agent.model ||
    description !== agent.description ||
    (goal ?? '') !== (agent.goal ?? '');

  const requestClose = (): void => {
    if (dirty && !window.confirm(t('editAgent.unsavedWarning'))) return;
    onClose();
  };

  // Esc = the backdrop click: same unsaved-changes confirm.
  useEscape(requestClose);

  const save = () => {
    const trimmedName = name.trim() || agent.name;
    const trimmedDescription = description.trim() || 'a fresh harness';
    const trimmedGoal = goal.trim();
    const command = config
      ? buildSpawnCommand(config, model, provider)
      : agent.command;

    updateAgent(agent.id, {
      name: trimmedName,
      character,
      accent,
      provider,
      model,
      command,
      description: trimmedDescription,
      goal: trimmedGoal || undefined
    });
    onClose();
  };

  const nameRow = (
    <Row label={t('addAgent.name')}>
                <input
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={t('editAgent.namePlaceholder')}
                  style={inputStyle}
                  autoFocus
                />
              </Row>
  );
  const characterRow = (
    <Row label={t('addAgent.character')}>
                <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                  {OFFICE_CAST.map((c) => {
                    const active = character === c.name;
                    return (
                      <button
                        key={c.name}
                        type="button"
                        onClick={() => {
                          setCharacter(c.name);
                          const isCastName = OFFICE_CAST.some(m => m.displayName === name || m.displayName === agent.name);
                          if (!name.trim() || name === agent.name || isCastName) setName(c.displayName);
                        }}
                        title={c.blurb}
                        style={{
                          padding: 4,
                          background: active ? `var(--cth-${accent}-light)` : 'var(--cth-cream-100)',
                          boxShadow: active
                            ? 'inset 0 0 0 1.5px var(--cth-ink-500)'
                            : 'inset 0 0 0 1px var(--cth-ink-100)',
                          cursor: 'pointer', border: 'none', width: 52,
                          display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 2
                        }}
                      >
                        <div style={{
                          width: 40, height: 48,
                          display: 'flex', alignItems: 'flex-end', justifyContent: 'center',
                          overflow: 'hidden'
                        }}>
                          <SpritePortrait character={c.name} scale={1.5} />
                        </div>
                        <span style={{ fontSize: 10, color: 'var(--cth-ink-700)' }}>{c.displayName}</span>
                      </button>
                    );
                  })}
                </div>
              </Row>
  );
  const colorRow = (
    <Row label={t('addAgent.color')}>
                <div style={{ display: 'flex', gap: 6 }}>
                  {ACCENTS.map((a) => (
                    <button
                      key={a}
                      type="button"
                      onClick={() => setAccent(a)}
                      title={a}
                      aria-label={a}
                      style={{
                        width: 28, height: 28,
                        background: `var(--cth-${a})`,
                        boxShadow: accent === a
                          ? 'inset 0 0 0 1.5px var(--cth-ink-500), 0 0 0 2px var(--cth-ink-900)'
                          : 'inset 0 0 0 1px var(--cth-ink-300)',
                        cursor: 'pointer', border: 'none'
                      }}
                    />
                  ))}
                </div>
              </Row>
  );
  const descriptionRow = (
    <Row label={t('editAgent.description')}>
                <input
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder={t('editAgent.descriptionPlaceholder')}
                  style={inputStyle}
                />
              </Row>
  );
  const goalRow = (
    <Row label={t('editAgent.goalLabel', sctx)}>
                <textarea
                  value={goal}
                  onChange={(e) => setGoal(e.target.value)}
                  placeholder={t('editAgent.goalPlaceholder', sctx)}
                  rows={4}
                  style={{ ...inputStyle, fontFamily: 'var(--cth-font-ui)', resize: 'vertical', minHeight: 200 }}
                />
              </Row>
  );
  const engineSection = (
    <Section label={t('addAgent.sections.engine.label')} hint={t('editAgent.engineHint')}>
              <Row label={t('editAgent.provider')}>
                <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                  {AGENT_PROVIDER_PRESETS.map((p) => {
                    const active = provider === p.id;
                    return (
                      <button
                        key={p.id}
                        type="button"
                        onClick={() => pickProvider(p.id)}
                        title={p.label}
                        style={{
                          padding: '3px 8px 1px',
                          background: active ? `var(--cth-${accent}-light)` : 'var(--cth-cream-100)',
                          boxShadow: active
                            ? 'inset 0 0 0 1.5px var(--cth-ink-500)'
                            : 'inset 0 0 0 1px var(--cth-ink-100)',
                          fontFamily: 'var(--cth-font-ui)', fontSize: 12,
                          color: 'var(--cth-ink-900)', cursor: 'pointer', border: 'none',
                          display: 'inline-flex', alignItems: 'center', gap: 6
                        }}
                      >
                        <ProviderLogo provider={p.id} size={14} />
                        {p.label}
                        {engineCostHint(p.id) && (
                          <span style={{
                            fontSize: 10, color: 'var(--cth-ink-500)',
                            fontWeight: engineCostHint(p.id)!.recommended ? 700 : 400
                          }}>
                            {t(engineCostHint(p.id)!.tagKey)}
                          </span>
                        )}
                      </button>
                    );
                  })}
                </div>
                {engineCostHint(provider) && (
                  <span data-testid="engine-cost-hint" style={{ fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-500)' }}>
                    {t(engineCostHint(provider)!.descKey)}
                  </span>
                )}
              </Row>

              {preset.supportsModel && (
                <Row label={t('editAgent.model')}>
                  {modelsForProvider(provider).length === 0 ? (
                    // Catalog-empty providers (hermes, openclaw, kimi…)
                    // take a freeform --model string; empty = CLI default.
                    <>
                      <input
                        value={model ?? ''}
                        onChange={(e) => setModel(e.target.value.trim() || undefined)}
                        placeholder={t('fullscreenTerminal.cliDefault')}
                        style={{ padding: '4px 8px', fontFamily: 'var(--cth-font-mono)', fontSize: 12, border: '1px solid var(--cth-ink-300)', background: 'var(--cth-cream-50)', color: 'var(--cth-ink-900)' }}
                      />
                      {capabilitiesForProvider(provider).modelHintKey && (
                        <span style={{ display: 'block', marginTop: 4, fontSize: 11, lineHeight: '15px', color: 'var(--cth-ink-500)' }}>
                          {t(capabilitiesForProvider(provider).modelHintKey!)}
                        </span>
                      )}
                    </>
                  ) : (
                  <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
                    {(() => {
                      const known = modelsForProvider(provider);
                      return model && !known.some((m) => m.id === model)
                        ? [...known, { id: model, label: t('editAgent.modelCurrent', { model }) }]
                        : known;
                    })().map((m) => {
                      const active = (model ?? '') === (m.id ?? '');
                      return (
                        <button
                          key={m.label}
                          type="button"
                          onClick={() => setModel(m.id)}
                          title={m.id ?? t('editAgent.modelDefaultTitle')}
                          style={{
                            padding: '3px 8px 1px',
                            background: active ? `var(--cth-${accent}-light)` : 'var(--cth-cream-100)',
                            boxShadow: active
                              ? 'inset 0 0 0 1.5px var(--cth-ink-500)'
                              : 'inset 0 0 0 1px var(--cth-ink-100)',
                            fontFamily: 'var(--cth-font-ui)', fontSize: 12,
                            color: 'var(--cth-ink-900)', cursor: 'pointer', border: 'none'
                          }}
                        >
                          {m.label}
                        </button>
                      );
                    })}
                  </div>
                  )}
                </Row>
              )}

              <span style={{ fontSize: 12, color: 'var(--cth-ink-500)', lineHeight: '16px' }}>
                {t('editAgent.engineNote')}
              </span>
              <ProviderNotes provider={provider} autoMode={!!config?.autoMode && !!preset.autoFlag} />
            </Section>
  );

  return (
    <div
      onClick={requestClose}
      style={{
        position: 'fixed', inset: 0,
        background: 'rgba(15, 28, 46, 0.6)',
        display: 'flex', alignItems: 'center', justifyContent: 'center',
        zIndex: 500
      }}
    >
      {/* Same box as Add Agent (940 / 95vw / 86vh). They are the two halves of
          one job — describe an agent — and a tall narrow dialog next to a wide
          one reads as two unrelated screens. */}
      <div onClick={(e) => e.stopPropagation()} style={{ width: simple ? 620 : 940, maxWidth: '95vw' }}>
        <PixelPanel variant="dialog" title={t('editAgent.title', sctx)} style={{ padding: 16 }} noPadding>
          <div style={{
            display: 'flex', flexDirection: 'column', gap: 14,
            padding: 16, maxHeight: '86vh', overflowY: 'auto'
          }}>
            {/* Two columns so the extra width is used rather than padded.
                Identity and Engine are short field lists; Briefing is free
                text and takes the taller side. minHeight keeps the dialog from
                collapsing into a wide thin strip on a small form. */}
            {simple && (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
                {nameRow}
                {goalRow}
                <button
                  type="button"
                  onClick={() => setMoreOpen((v) => !v)}
                  aria-expanded={moreOpen}
                  data-testid="edit-agent-more"
                  style={{
                    alignSelf: 'flex-start', padding: '4px 0', border: 'none', background: 'transparent',
                    cursor: 'pointer', fontFamily: 'var(--cth-font-ui)', fontSize: 13,
                    color: 'var(--cth-ink-700)', textDecoration: 'underline'
                  }}
                >
                  {moreOpen ? '\u25be' : '\u25b8'} {t('simpleMode.addAgent.moreOptions')}
                </button>
                {moreOpen && (
                  <>
                    {characterRow}
                    {colorRow}
                    {engineSection}
                    {descriptionRow}
                  </>
                )}
              </div>
            )}

            {!simple && (
            <div style={{
              display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))',
              gap: 16, alignItems: 'start', minHeight: 260
            }}>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 14, minWidth: 0 }}>
            <Section label={t('addAgent.sections.identity.label')} hint={t('addAgent.sections.identity.hint')}>
              {nameRow}

              {characterRow}

              {colorRow}
            </Section>

            {engineSection}

              </div>
              <div style={{ minWidth: 0 }}>
            <Section label={t('addAgent.sections.briefing.label')} hint={t('addAgent.sections.briefing.hint')}>
              {descriptionRow}

              {goalRow}
            </Section>
              </div>
            </div>

            )}

            <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end', marginTop: 4 }}>
              <PixelButton variant="ghost" size="md" onClick={requestClose}>{t('common.cancel')}</PixelButton>
              <div style={{ flex: 1 }} />
              <PixelButton variant="primary" size="md" onClick={save}>{t('editAgent.saveChanges', sctx)}</PixelButton>
            </div>
          </div>
        </PixelPanel>
      </div>
    </div>
  );
}

const inputStyle: CSSProperties = {
  width: '100%',
  padding: '6px 8px 4px',
  background: 'var(--cth-paper-100)',
  border: 'none',
  boxShadow: 'inset 0 0 0 1px var(--cth-ink-100)',
  fontFamily: 'var(--cth-font-ui)',
  fontSize: 16,
  color: 'var(--cth-ink-900)',
  outline: 'none',
  boxSizing: 'border-box'
};

function Section({
  label,
  hint,
  children
}: {
  label: string;
  hint: string;
  children: React.ReactNode;
}) {
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
      <div style={{ display: 'flex', alignItems: 'baseline', gap: 8 }}>
        <span style={{
          fontFamily: 'var(--cth-font-display)',
          fontSize: 9, lineHeight: '12px',
          color: 'var(--cth-ink-900)',
          textTransform: 'uppercase'
        }}>{label}</span>
        <span style={{ fontSize: 11, color: 'var(--cth-ink-500)' }}>{hint}</span>
      </div>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <span style={{
        fontFamily: 'var(--cth-font-display)',
        fontSize: 8, lineHeight: '12px',
        color: 'var(--cth-ink-700)',
        textTransform: 'uppercase'
      }}>{label}</span>
      {children}
    </label>
  );
}
