import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore, selectedAgent } from '@/store/store';
import { startMockLoop, stopMockLoop } from '@/store/mockEvents';
import type { HarnessConfig } from '@/store/config';
import { DEFAULT_ORG_TRIGGER } from '@shared/triggers';
import { OfficeFloor } from '@/scene/office/OfficeFloor';
import { useHive } from '@/hooks/useHive';
import { useRestoreTeam, dismissRestorePrompt } from '@/hooks/useRestoreTeam';
import { useResolvedGodName } from '@/hooks/useResolvedGodName';
import { useGodNameSync } from '@/i18n/useGodNameSync';
import { useDirectionSync } from '@/i18n/useDirection';
import { useArabicTerminalSync } from '@/terminal/useArabicTerminalSync';
import { MemoryPanel } from '@/components/MemoryPanel';
import { AgentDetailPanel } from '@/components/AgentDetailPanel';
import { AgentStrip } from '@/components/AgentStrip';
import { AddAgentModal } from '@/components/AddAgentModal';
import { MichaelBooting } from '@/components/MichaelBooting';
import { OnboardingWizard } from '@/components/OnboardingWizard';
import { HivePicker } from '@/components/HivePicker';
import { QuitWarningModal, type ClosingTimeState } from '@/components/QuitWarningModal';
import { CompletionToast } from '@/realtime/CompletionToast';
import { UpdateToast } from '@/components/UpdateToast';
import { UpdateBadge } from '@/components/UpdateBadge';
import { useAppTheme, toggleAppTheme } from '@/design/theme';
import { SettingsModal, type Section as SettingsSection } from '@/components/SettingsModal';
import { PixelPanel } from '@/components/PixelPanel';
import { PixelButton } from '@/components/PixelButton';
import { Icon } from '@/components/Icon';
import { SidebarSplitter } from '@/components/SidebarSplitter';
import { acquireTerminal, notifyThemeChangeAll } from '@/components/terminalPool';
import { FullscreenTerminal } from '@/components/FullscreenTerminal';
import { TaskDetailOverlay } from '@/components/TaskDetailOverlay';
import { IdePanel } from '@/ide/IdePanel';
import { useHoldOptionToTalk } from '@/freeflow/holdOption';
import { FloorCloseModal } from '@/components/FloorCloseModal';
import { floorNumber, isFloorWindow, isNewFloorShortcut, newFloorEnabled } from '@/floors';
import brandLogo from '@brand/logo.png?url';

// Injected at build time from package.json (see electron.vite.config.ts).
declare const __APP_VERSION__: string;

export function App() {
  const { t } = useTranslation();
  // Point every {{godName}} string at the orchestrator's real, renameable name.
  useGodNameSync();
  // Mirror the document only for a user who has picked an RTL app language.
  useDirectionSync();
  // Let terminals that are ALREADY open follow a language switch too.
  useArabicTerminalSync();
  const agent = useStore(selectedAgent);
  const agents = useStore(s => s.agents);
  const agentCount = agents.length;
  const bootingGodName = useResolvedGodName();
  const addAgentOpen = useStore(s => s.addAgentOpen);
  const setAddAgentOpen = useStore(s => s.setAddAgentOpen);
  const clearPendingHires = useStore(s => s.clearPendingHires);
  const godStatus = useStore(s => s.godStatus);
  const fullscreenAgentId = useStore(s => s.fullscreenAgentId);
  const appThemeNow = useAppTheme();
  const sidebarWidth = useStore(s => s.sidebarWidth);
  const setSidebarWidth = useStore(s => s.setSidebarWidth);
  const ideOpen = useStore(s => s.ideOpen);
  const setIdeOpen = useStore(s => s.setIdeOpen);

  const [config, setConfig] = useState<HarnessConfig | null>(null);
  // Whether the user has passed the launch-time hive picker this session. Starts
  // true (skip the picker) right after a hive SWITCH — changeHome relaunches and
  // leaves a one-shot localStorage flag so we don't bounce back onto the picker for
  // the hive we just chose. Also set true on onboarding completion (below).
  const [hiveOpened, setHiveOpened] = useState<boolean>(() => {
    // A floor is opened from inside an already-open hive — never re-ask.
    if (isFloorWindow()) return true;
    try {
      if (window.localStorage.getItem('cth.skipHivePickerOnce')) {
        window.localStorage.removeItem('cth.skipHivePickerOnce');
        return true;
      }
    } catch { /* localStorage unavailable — show the picker */ }
    return false;
  });
  const [settingsOpen, setSettingsOpen] = useState(false);
  /** Which tab Settings opens on. Set by a `cth:open-settings` deep link, reset
   *  to undefined (→ General) whenever the modal is opened the normal way. */
  const [settingsSection, setSettingsSection] = useState<SettingsSection | undefined>(undefined);
  const [quitWarn, setQuitWarn] = useState<{ ptyCount: number } | null>(null);
  const [closing, setClosing] = useState<ClosingTimeState | null>(null);
  const [vpWidth, setVpWidth] = useState<number>(window.innerWidth);
  // Why the first config read failed (null = loading / fine), and a counter that
  // re-runs the load effect for the Retry button.
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  // A dismissible banner for failures the user would otherwise never see
  // (a deep-link hire that could not be imported).
  const [notice, setNotice] = useState<string | null>(null);

  // Failures that happen deep in the tree with no UI of their own (a queued
  // message that could not be delivered) surface through the banner below.
  useEffect(() => {
    const onNotice = (e: Event): void => {
      const text = (e as CustomEvent<{ text?: string }>).detail?.text;
      if (text) setNotice(text);
    };
    window.addEventListener('cth:notice', onNotice);
    return () => window.removeEventListener('cth:notice', onNotice);
  }, []);

  // Deep link into Settings from anywhere in the tree. Settings' open state is
  // local to App, so a nested control (e.g. "set it now" beside a disabled Talk
  // button) has no path to it without threading a prop through every layer
  // between; a window event keeps that plumbing out of the components in
  // between, matching the existing `cth:` CustomEvent convention.
  useEffect(() => {
    const onOpenSettings = (e: Event): void => {
      const section = (e as CustomEvent<{ section?: SettingsSection }>).detail?.section;
      setSettingsSection(section);
      setSettingsOpen(true);
    };
    window.addEventListener('cth:open-settings', onOpenSettings);
    return () => window.removeEventListener('cth:open-settings', onOpenSettings);
  }, []);

  // Adopt a FULL config read: the state plus the store mirrors derived from it.
  const applyConfig = useCallback((c: HarnessConfig) => {
    setConfig(c);
    // Mirror the Free Flow flag into the store so the composer mic button shows
    // only when enabled (Settings keeps this in sync on save).
    // On unless explicitly turned off — the same default Settings shows.
    useStore.getState().setFreeflowEnabled(c.freeflowEnabled !== false);
    // Mirror boolean key-presence ONLY (never the key value) so the composer can
    // show the voice button disabled-with-tooltip when Free Flow is on but no
    // Groq key is set (Settings keeps this in sync on save).
    useStore.getState().setHasGroqKey(!!c.groqApiKey);
    // Mirror the active office theme so OfficeFloor renders it (gated on the
    // tvShowOffices flag; off = always the office). Settings keeps this synced.
    useStore.getState().setOfficeTheme(c.tvShowOffices ? (c.officeTheme ?? 'office') : 'office');
    // Mirror the triggers so Settings → Connections and the Command Center's
    // Triggers tab read one list, not two copies that drift — whichever surface
    // saves calls these same setters and the other repaints. No extra IPC: main
    // deep-fills both fields on every config read (withTriggerDefaults), so
    // getConfig() already serves what listWebhooks()/getOrgTrigger() would.
    // `c` is typed as the PRELOAD's HarnessConfig, which hasn't picked the two
    // fields up yet (another lane's file); the renderer mirror type declares them.
    const withTriggers = c as HarnessConfig;
    useStore.getState().setWebhookTriggers(withTriggers.webhookTriggers ?? []);
    useStore.getState().setOrgTrigger(withTriggers.orgTrigger ?? DEFAULT_ORG_TRIGGER);
  }, []);

  // Initial config load
  useEffect(() => {
    let cancelled = false;
    setLoadError(null);
    window.cth.getConfig().then(c => {
      if (!cancelled) applyConfig(c);
    }).catch((err: unknown) => {
      // A rejected first read used to leave a blank cream screen forever — say
      // what failed and let the user retry.
      if (!cancelled) setLoadError(err instanceof Error ? err.message : String(err));
    });
    // Mirror BYOK OpenAI key presence (boolean only; the key never leaves main) so the
    // Realtime Michael voice toggle can gate on it. Lives in the secret broker, not
    // config — so fetch it rather than derive from c.
    window.cth.realtimeHasOpenAiKey().then(has => {
      if (!cancelled) useStore.getState().setHasOpenAiKey(has);
    }).catch(() => { /* key presence stays "no" — the voice toggle just reads disabled */ });
    return () => { cancelled = true; };
  }, [applyConfig, loadAttempt]);

  // Free Flow entry point B — hold-Option (⌥) to talk. In-renderer push-to-talk
  // for whichever agent the user is viewing; gated on the flag, terminal-safe
  // (solo-hold threshold, aborts on any other key). See freeflow/holdOption.ts.
  useHoldOptionToTalk();

  // Config subscription — the copy loaded above would otherwise go stale the
  // moment anything saves a setting. The backend emits PARTIAL payloads ({} after
  // a voice set-setting action, {autoMode} from set_auto_mode); replacing the
  // whole config with one dropped onboardingComplete and bounced the user back
  // into the wizard. Merge what the payload carries, and when it is not a full
  // config re-read so the store mirrors (voice/Free Flow flags) refresh too.
  useEffect(() => window.cth.onConfigChanged((payload) => {
    const patch = (payload && typeof payload === 'object' ? payload : {}) as Partial<HarnessConfig>;
    setConfig((prev) => (prev ? { ...prev, ...patch } : prev));
    if (!('onboardingComplete' in patch)) {
      window.cth.getConfig().then((c) => applyConfig(c)).catch(() => { /* the merge above stands */ });
    }
  }), [applyConfig]);

  // Quit warning subscription
  useEffect(() => window.cth.onCloseRequested((info) => setQuitWarn(info)), []);

  // Multi-window floors. A floor that still owns live terminals is asked to
  // close → confirm here (stop ITS agents and close, or keep it open).
  const [floorClose, setFloorClose] = useState<{ ptyCount: number } | null>(null);
  useEffect(() => window.cth.onFloorCloseRequested?.((info) => setFloorClose(info)), []);
  const multiWindowOn = newFloorEnabled(config);
  const openNewFloor = useCallback(async () => {
    try {
      const res = await window.cth.newFloor();
      if (res && res.ok === false) setNotice(res.error ?? t('floor.disabled'));
    } catch (err) {
      setNotice(t('floor.newFailed', { error: err instanceof Error ? err.message : String(err) }));
    }
  }, [t]);
  // Ctrl+Shift+N. Capture phase: xterm would otherwise swallow it as a key for
  // the terminal's own process.
  useEffect(() => {
    if (!multiWindowOn) return;
    const onKey = (e: KeyboardEvent): void => {
      if (!isNewFloorShortcut(e)) return;
      e.preventDefault();
      e.stopPropagation();
      void openNewFloor();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [multiWindowOn, openNewFloor]);

  // Shareable hires: a validated manifest arriving via the agent-office://
  // deep link (or file import) pre-fills the Add-Agent modal. Never spawns by itself.
  const enqueuePendingHires = useStore(s => s.enqueuePendingHires);
  const closeAddAgentReview = () => {
    clearPendingHires();
    setAddAgentOpen(false);
  };
  useEffect(() => {
    let alive = true;
    // The deep link carries a manifest SOURCE (https URL): the backend fetches +
    // validates it, then the manifest joins the review queue.
    const importSource = (src: string) => {
      window.cth.hireFetch(src).then((res) => {
        if (!alive) return;
        if (res?.ok && res.manifest) {
          enqueuePendingHires([res.manifest]);
          setAddAgentOpen(true);
        } else {
          setNotice(t('app.hireFailed', { error: t('app.hireInvalid') }));
        }
      }).catch((err: unknown) => {
        if (alive) setNotice(t('app.hireFailed', { error: err instanceof Error ? err.message : String(err) }));
      });
    };
    // The backend parks every deep-link source on disk AND emits it; draining
    // clears the parked copy so it is not imported a second time next launch.
    const drain = (live?: string) => {
      Promise.resolve(window.cth.drainPendingHires?.()).catch(() => null).then((parked) => {
        const sources = new Set<string>();
        if (typeof parked === 'string' && parked) sources.add(parked);
        if (live) sources.add(live);
        sources.forEach(importSource);
      });
    };
    const unsub = window.cth.onHirePending?.((src) => drain(typeof src === 'string' ? src : undefined));
    // Pull anything that arrived before this subscription existed (cold-start
    // deep links; packaged renderers load too fast for push-on-load).
    drain();
    return () => { alive = false; unsub?.(); };
  }, [enqueuePendingHires, setAddAgentOpen, t]);

  // Closing-time progress: drives the quit dialog's "wrapping up" view. The
  // dialog stays up through the whole protocol; on 'complete' the main process
  // tears down and quits by itself moments later.
  useEffect(() => window.cth.onClosingTime?.((ev) => {
    if (ev.phase === 'cancelled') { setClosing(null); return; }
    setClosing({ phase: ev.phase, acked: ev.acked, total: ev.total });
    if (ev.phase === 'started' || ev.phase === 'progress') setQuitWarn((w) => w ?? { ptyCount: 0 });
  }), []);

  const startClosingTime = async () => {
    setClosing(null); // a retry clears the previous failure
    try {
      // Resolves the live worker count once the protocol is armed; only a
      // rejection (e.g. no god running to brief) is a failure.
      await window.cth.startClosingTime();
    } catch (err) {
      // A throwing bridge must not leave the user unable to close the app —
      // surface the real reason as a failed closing phase, not a silent dead modal.
      setClosing({
        phase: 'error', acked: 0, total: 0,
        error: err instanceof Error ? err.message : String(err)
      });
    }
  };
  const cancelClosingTime = () => {
    void Promise.resolve(window.cth.cancelClosingTime()).catch(() => { /* nothing armed */ });
    setClosing(null);
  };

  // The hive: god-agent bootstrap, hook-driven avatars, idle-agent waking. Held
  // off until the user opens a hive in the launch picker (passing null no-ops the
  // hook) so Michael doesn't boot against the current home while the user may be
  // about to switch to a different one.
  useHive(hiveOpened ? config : null);

  // Last session's team: at boot the hook only ASKS (see the prompt below) —
  // nothing is respawned until the user presses Restore.
  const { restorePrompt, restoreTeam } = useRestoreTeam(hiveOpened ? config : null);
  const restorableAgents = useStore((s) => s.restorableAgents);

  // Pre-warm a persistent terminal for every live agent so its output is
  // buffered from spawn. Switching agents then re-attaches an already-rendered
  // terminal instantly (with full history) instead of building a blank one.
  useEffect(() => {
    for (const a of agents) if (a.ptyId) acquireTerminal(a.ptyId);
  }, [agents]);

  // Synthetic demo loop — CAGED (#5B). It must never animate alongside a live
  // hive (it would fire fake envelope handoffs and step seeded agents). Run it
  // only as an explicit showcase (VITE_CTH_DEMO=1 in dev) or on a genuinely
  // empty floor, and stop it the instant the first real PTY agent appears
  // (Michael always spawns, so in normal operation it effectively never runs).
  useEffect(() => {
    if (!config?.onboardingComplete) return;
    const DEMO = import.meta.env.DEV && import.meta.env.VITE_CTH_DEMO === '1';
    const evaluate = () => {
      const hasLive = useStore.getState().agents.some((a) => a.ptyId);
      if (DEMO || !hasLive) startMockLoop();
      else stopMockLoop();
    };
    evaluate();
    const unsub = useStore.subscribe(evaluate);
    return () => { unsub(); stopMockLoop(); };
  }, [config?.onboardingComplete]);

  // Reconcile restored agents against the PTYs still alive in the main process.
  // After a renderer reload (e.g. the laptop slept and Vite reloaded the page),
  // this keeps agents whose process survived and drops any that truly died.
  useEffect(() => {
    if (!config?.onboardingComplete) return;
    let cancelled = false;
    window.cth.listPtys().then((list) => {
      if (cancelled) return;
      useStore.getState().reconcileWithLivePtys(list.map((p) => p.id));
    }).catch(() => { /* ignore — keep restored agents as-is */ });
    return () => { cancelled = true; };
  }, [config?.onboardingComplete]);

  // Re-apply the persisted focus-mode preference as the roster fills in.
  //
  // Not a one-shot at store construction: at launch every restored agent still
  // carries the PREVIOUS session's PTY id, so the reconcile above prunes the lot
  // and correctly drops focus mode to null before god has respawned. The
  // preference therefore has to be re-checked once agents with live terminals
  // actually exist. `restoreFocusMode` is a no-op unless the preference is on and
  // focus mode is currently off, so re-running it on every roster change is safe
  // and pressing Esc stays sticky.
  useEffect(() => {
    if (!config?.onboardingComplete) return;
    useStore.getState().restoreFocusMode();
  }, [config?.onboardingComplete, agents]);

  // Track viewport width for splitter clamping
  useEffect(() => {
    const onResize = () => setVpWidth(window.innerWidth);
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  if (!config) {
    if (loadError) {
      return (
        <div style={{
          width: '100vw', height: '100vh', background: 'var(--cth-cream-100)',
          display: 'flex', alignItems: 'center', justifyContent: 'center'
        }}>
          <div style={{ width: 420, maxWidth: '92vw' }}>
            <PixelPanel variant="dialog" title={t('app.loadFailedTitle')} noPadding>
              <div role="alert" style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 10 }}>
                <p style={{ margin: 0, fontSize: 13, lineHeight: '20px' }}>{t('app.loadFailedBody')}</p>
                <code style={{ fontSize: 12, wordBreak: 'break-word', color: 'var(--cth-ink-700)' }}>{loadError}</code>
                <PixelButton variant="primary" size="md" onClick={() => setLoadAttempt((n) => n + 1)}>
                  {t('app.retry')}
                </PixelButton>
              </div>
            </PixelPanel>
          </div>
        </div>
      );
    }
    return <div style={{ width: '100vw', height: '100vh', background: 'var(--cth-cream-100)' }} />;
  }

  if (!config.onboardingComplete) {
    // Just-onboarded users go straight into the hive they set up — skip the picker.
    return <OnboardingWizard onComplete={(next) => { setConfig(next); setHiveOpened(true); }} />;
  }

  // Launch-time hive picker: on reopen, let the user open their current hive,
  // switch to a recent one, or open/create another. Skipped right after onboarding
  // and right after a switch-relaunch (see hiveOpened init).
  if (!hiveOpened) {
    return <HivePicker config={config} onOpenCurrent={() => setHiveOpened(true)} />;
  }

  return (
    <div style={{
      display: 'flex', flexDirection: 'column',
      width: '100vw', height: '100vh',
      overflow: 'hidden'
    }}>
      {/* rt-12: global fixed-overlay toast for voice-Michael completions ("Oscar
          finished X"). Self-positions bottom-right; renders null until one arrives. */}
      <CompletionToast />
      {/* v0.3.4: background-update toast ("restart to update"); renders null until
          main's updater pushes a status. */}
      <UpdateToast />
      {/* Title bar */}
      <div
        className="cth-titlebar-drag"
        style={{
          height: 36, minHeight: 36,
          background: 'linear-gradient(180deg, var(--cth-cream-100) 0%, var(--cth-cream-200) 100%)',
          borderBottom: '1px solid var(--cth-ink-300)',
          display: 'flex',
          alignItems: 'center',
          // macOS draws its traffic lights over this bar; Windows/Linux keep the
          // native frame above it, so there is no gutter to reserve there.
          paddingLeft: window.cth.platform === 'darwin' ? 96 : 12,
          paddingRight: 12,
          gap: 12,
          userSelect: 'none'
        }}
      >
        <img
          src={brandLogo}
          alt={t('app.productName')}
          style={{ height: 20, width: 'auto', display: 'block' }}
        />
        {/* v0.3.7: the version is no longer inert text — it doubles as the
            update control (check / download / restart to update). */}
        <UpdateBadge />
        {isFloorWindow() && (
          <span
            data-testid="floor-badge"
            style={{
              fontFamily: 'var(--cth-font-display)', fontSize: 10, lineHeight: '14px',
              padding: '2px 6px', color: 'var(--cth-ink-900)',
              background: 'var(--cth-lemon-light, #f6ecc4)',
              boxShadow: 'inset 0 0 0 1px var(--cth-ink-500)'
            }}
          >
            {t('app.floorBadge', { n: floorNumber() })}
          </span>
        )}
        <span style={{
          fontFamily: 'var(--cth-font-ui)',
          fontSize: 13,
          color: 'var(--cth-ink-500)'
        }}>
          {config.autoMode ? t('app.autoModeOn') : t('app.autoModeOff')}
        </span>
        {/* v0.3.4: theme + fullscreen live HERE (top right), not buried in the
            terminal header — and the theme darkens the whole app, terminals
            included (design/theme.ts + tokens.css dark block). */}
        <button
          className="cth-titlebar-nodrag cth-tip"
          onClick={() => {
            const next = toggleAppTheme();
            // Tell every RUNNING program the theme flipped. xterm repaints its own
            // cells, but a TUI that painted its panels with explicit colours keeps
            // them until it redraws, which left OpenCode's boxes in the old palette
            // until the agent restarted. Only programs that enabled DEC mode 2031
            // are told, and it is every pooled terminal rather than the visible one,
            // so a background agent is not stale when you switch to it.
            notifyThemeChangeAll(next === 'dark' ? 'dark' : 'light');
            // Mirror into the harness config: every agent (re)spawned from now
            // on gets the matching `theme` in its per-session Claude settings,
            // so the TUI's truecolor palette fits the terminal. Scoped to
            // harness agents — the user's global Claude theme is never touched.
            void window.cth.updateConfig({ terminalTheme: next });
          }}
          data-tip={appThemeNow === 'dark' ? t('app.lightTheme') : t('app.darkTheme')}
          aria-label={t('app.toggleDarkMode')}
          style={{
            marginLeft: 'auto',
            display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
            width: 28, height: 28, padding: 0,
            background: 'var(--cth-paper-100)',
            boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
            border: 'none', borderRadius: 2, cursor: 'pointer',
            color: 'var(--cth-ink-900)', fontSize: 13, lineHeight: 1
          }}
        >
          {appThemeNow === 'dark' ? '☀' : '☾'}
        </button>
        {/* v0.3.4: the IDE button moved to agent level — every agent's header
            (sidebar detail, god Command Center, fullscreen) carries it. */}
        <button
          className="cth-titlebar-nodrag cth-settings-btn cth-tip"
          onClick={() => { setSettingsSection(undefined); setSettingsOpen(true); }}
          data-tip={t('app.settings')}
          aria-label={t('app.settings')}
          style={{
            display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
            width: 28, height: 28, padding: 0,
            background: 'var(--cth-paper-100)',
            boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
            border: 'none', borderRadius: 2, cursor: 'pointer',
            color: 'var(--cth-ink-900)'
          }}
        >
          <GearGlyph />
        </button>
        {/* Fullscreen. The title bar is chrome, not canvas, so these two use
            clean stroke icons rather than the 16x16 pixel set the rest of the UI
            is drawn in — at 16-18px a pixel-grid glyph reads as a rendering
            artifact next to the OS window controls, not as a style choice. */}
        <button
          className="cth-titlebar-nodrag cth-tip"
          onClick={() => {
            if (fullscreenAgentId) { useStore.getState().setFullscreen(null); return; }
            const all = useStore.getState().agents;
            const target = all.find((x) => x.id === useStore.getState().selectedId && x.ptyId)
              ?? all.find((x) => x.isGod && x.ptyId)
              ?? all.find((x) => x.ptyId);
            if (target) useStore.getState().setFullscreen(target.id);
          }}
          data-tip={fullscreenAgentId ? t('app.exitFocusMode') : t('app.focusMode')}
          aria-label={t('app.toggleFocusMode')}
          style={{
            display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
            width: 28, height: 28, padding: 0,
            background: 'var(--cth-paper-100)',
            boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
            border: 'none', borderRadius: 2, cursor: 'pointer',
            color: 'var(--cth-ink-900)'
          }}
        >
          {fullscreenAgentId ? <CollapseGlyph /> : <ExpandGlyph />}
        </button>

        {/* Multi-window floors (experimental, Settings → General): only while
            the flag is on. Ctrl+Shift+N does the same. */}
        {multiWindowOn && (
          <button
            className="cth-titlebar-nodrag cth-tip"
            onClick={() => { void openNewFloor(); }}
            data-tip={t('app.newFloor')}
            aria-label={t('app.newFloor')}
            style={{
              display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
              width: 28, height: 28, padding: 0,
              background: 'var(--cth-paper-100)',
              boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
              border: 'none', borderRadius: 2, cursor: 'pointer',
              color: 'var(--cth-ink-900)'
            }}
          >
            <NewFloorGlyph />
          </button>
        )}

      </div>

      <div style={{
        flex: 1, minHeight: 0,
        display: 'flex',
        padding: 16,
        gap: 0
      }}>
        <div style={{ flex: 1, minHeight: 0, minWidth: 0, position: 'relative' }}>
          <OfficeFloor />
          <MemoryPanel />
          {agentCount === 0 && godStatus === 'booting' && <MichaelBooting />}
          {agentCount === 0 && godStatus !== 'booting' && (
            <div style={{
              position: 'absolute', inset: 0,
              display: 'flex', alignItems: 'center', justifyContent: 'center',
              pointerEvents: 'none'
            }}>
              <div style={{ pointerEvents: 'auto', width: 360 }}>
                <PixelPanel variant="dialog" title={t('app.emptyFloorTitle')} noPadding>
                  <div style={{ padding: 16, display: 'flex', flexDirection: 'column', gap: 10 }}>
                    <p style={{ margin: 0, fontSize: 13, lineHeight: '20px' }}>
                      {t('app.emptyFloorBody')}
                    </p>
                    <PixelButton variant="primary" size="md" onClick={() => setAddAgentOpen(true)}>
                      <span style={{ display: 'inline-flex', gap: 6, alignItems: 'center' }}>
                        <Icon name="plus" /> {t('app.addAgent')}
                      </span>
                    </PixelButton>
                  </div>
                </PixelPanel>
              </div>
            </div>
          )}
        </div>

        <SidebarSplitter
          width={sidebarWidth}
          onChange={setSidebarWidth}
          viewportWidth={vpWidth}
        />

        <div style={{
          width: sidebarWidth, flexShrink: 0,
          minHeight: 0, display: 'flex', flexDirection: 'column'
        }}>
          {agent ? (
            <AgentDetailPanel agent={agent} />
          ) : godStatus === 'booting' ? (
            <PixelPanel variant="default" noPadding style={{
              padding: 16, height: '100%',
              display: 'flex', flexDirection: 'column',
              justifyContent: 'center', alignItems: 'center', gap: 12
            }}>
              <div style={{
                fontFamily: 'var(--cth-font-display)', fontSize: 10, lineHeight: '14px',
                color: 'var(--cth-ink-500)'
              }}>{t('app.wakingFloor')}</div>
              <p style={{ margin: 0, fontSize: 13, textAlign: 'center', color: 'var(--cth-ink-700)' }}>
                {t('app.wakingDesc', { godName: bootingGodName })}
              </p>
            </PixelPanel>
          ) : (
            <PixelPanel variant="default" noPadding style={{
              padding: 16, height: '100%',
              display: 'flex', flexDirection: 'column',
              justifyContent: 'center', alignItems: 'center', gap: 12
            }}>
              <div style={{
                fontFamily: 'var(--cth-font-display)', fontSize: 10, lineHeight: '14px',
                color: 'var(--cth-ink-500)'
              }}>{t('app.noAgentSelected')}</div>
              <p style={{ margin: 0, fontSize: 13, textAlign: 'center', color: 'var(--cth-ink-700)' }}>
                {t('app.noAgentDesc')}
              </p>
              <PixelButton variant="secondary" size="md" onClick={() => setAddAgentOpen(true)}>
                <span style={{ display: 'inline-flex', gap: 6, alignItems: 'center' }}>
                  <Icon name="plus" /> {t('app.addAgent')}
                </span>
              </PixelButton>
            </PixelPanel>
          )}
        </div>
      </div>

      <AgentStrip config={config} />

      {addAgentOpen && (
        <AddAgentModal
          onClose={closeAddAgentReview}
          config={config}
          onConfigChange={setConfig}
        />
      )}

      {settingsOpen && (
        <SettingsModal
          config={config}
          initialSection={settingsSection}
          onClose={() => { setSettingsOpen(false); setSettingsSection(undefined); }}
        />
      )}

      {quitWarn && (
        <QuitWarningModal
          ptyCount={quitWarn.ptyCount}
          closing={closing}
          onCancel={() => {
            if (closing) cancelClosingTime();
            void Promise.resolve(window.cth.cancelClose()).catch(() => { /* nothing armed */ });
            setQuitWarn(null);
          }}
          // "Kill N & quit" / "force quit now": the backend kills every PTY and
          // exits. The modal shows a rejection and re-enables its buttons.
          onConfirm={() => window.cth.forceQuit()}
          onClosingTime={startClosingTime}
        />
      )}

      {floorClose && (
        <FloorCloseModal
          ptyCount={floorClose.ptyCount}
          onCancel={() => setFloorClose(null)}
          // Stops only this floor's PTYs and destroys the window; a rejection
          // is shown by the modal.
          onConfirm={() => window.cth.closeFloor()}
        />
      )}

      {notice && (
        <div
          role="alert"
          style={{
            position: 'fixed', top: 44, left: '50%', transform: 'translateX(-50%)',
            zIndex: 900, maxWidth: '80vw',
            display: 'flex', alignItems: 'flex-start', gap: 10,
            padding: '8px 12px',
            background: 'var(--cth-coral-light)',
            boxShadow: 'inset 0 0 0 1.5px var(--cth-ink-500)',
            fontSize: 13, lineHeight: '20px', color: 'var(--cth-ink-900)'
          }}
        >
          <span style={{ wordBreak: 'break-word' }}>{notice}</span>
          <button
            onClick={() => setNotice(null)}
            aria-label={t('common.close')}
            style={{
              border: 'none', background: 'transparent', cursor: 'pointer',
              color: 'var(--cth-ink-900)', fontSize: 14, lineHeight: '20px', padding: 0
            }}
          >✕</button>
        </div>
      )}

      {restorePrompt && restorableAgents.length > 0 && !quitWarn && (
        <div
          role="alertdialog"
          aria-label={t('restorePrompt.title')}
          style={{
            position: 'fixed', bottom: 16, left: '50%', transform: 'translateX(-50%)',
            zIndex: 900, maxWidth: '90vw',
            display: 'flex', alignItems: 'center', gap: 12, flexWrap: 'wrap',
            padding: '10px 14px',
            background: 'var(--cth-paper-100)',
            boxShadow: 'inset 0 0 0 1.5px var(--cth-ink-500), 4px 4px 0 var(--cth-ink-300)',
            fontSize: 13, lineHeight: '20px', color: 'var(--cth-ink-900)'
          }}
        >
          <span style={{ wordBreak: 'break-word' }}>
            {t('restorePrompt.question', {
              n: restorableAgents.length,
              names: restorableAgents.map((a) => a.name).join(', ')
            })}
          </span>
          <PixelButton variant="primary" size="sm" onClick={() => { void restoreTeam(); }}>
            {t('restorePrompt.restore')}
          </PixelButton>
          <PixelButton variant="secondary" size="sm" onClick={dismissRestorePrompt}>
            {t('restorePrompt.skip')}
          </PixelButton>
        </div>
      )}

      {fullscreenAgentId && <FullscreenTerminal config={config} />}
      {ideOpen && <IdePanel />}
      <TaskDetailOverlay />
    </div>
  );
}

/* ── Title-bar glyphs ────────────────────────────────────────────────────────
   Stroke icons on a 16 unit box, inheriting `currentColor` so they follow the
   theme exactly as the pixel set does. Deliberately NOT added to
   components/Icon.tsx: that library is the app's pixel-art identity and is used
   at tab and card scale, where the pixel grid is the point. These three sit
   beside the OS traffic lights, which is the one place that identity reads as a
   blurry asset rather than a decision. */
function Glyph({ children }: { children: React.ReactNode }) {
  return (
    <svg
      width="16" height="16" viewBox="0 0 16 16" fill="none"
      stroke="currentColor" strokeWidth={1.4}
      strokeLinecap="round" strokeLinejoin="round"
      aria-hidden="true" focusable="false"
    >{children}</svg>
  );
}

/** Four outward corner brackets — enter fullscreen. */
function ExpandGlyph() {
  return (
    <Glyph>
      <path d="M6.2 3H3v3.2M9.8 3H13v3.2M6.2 13H3V9.8M9.8 13H13V9.8" />
    </Glyph>
  );
}

/** Two stacked window outlines with a plus — open another floor. */
function NewFloorGlyph() {
  return (
    <Glyph>
      <path d="M5.5 5.5V3.5h8v6h-2M2.5 5.5h8v7h-8zM6.5 7.2v3.6M4.7 9h3.6" />
    </Glyph>
  );
}

/** The same brackets turned inward — leave fullscreen. */
function CollapseGlyph() {
  return (
    <Glyph>
      <path d="M3 6.2h3.2V3M13 6.2H9.8V3M3 9.8h3.2V13M13 9.8H9.8V13" />
    </Glyph>
  );
}

/** A wrench. The previous glyph was a hub with eight radiating spokes, which at
 *  18px is indistinguishable from a sun — sitting immediately beside a theme
 *  toggle whose light-mode icon IS a sun. A tool shape carries "settings"
 *  without competing with its neighbour. Drawn on a 24 box for curve headroom
 *  and rendered at 16. */
function GearGlyph() {
  return (
    <svg
      width="16" height="16" viewBox="0 0 24 24" fill="none"
      stroke="currentColor" strokeWidth={2}
      strokeLinecap="round" strokeLinejoin="round"
      aria-hidden="true" focusable="false"
    >
      <path d="M15.5 3.5a5 5 0 0 0-6.1 6.1l-5.6 5.6a2.3 2.3 0 1 0 3.2 3.2l5.6-5.6a5 5 0 0 0 6.1-6.1l-3 3-2.2-.6-.6-2.2z" />
    </svg>
  );
}
