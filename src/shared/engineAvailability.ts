/**
 * Can the engine a user is about to pick actually boot on this machine?
 *
 * The onboarding wizard records Michael's engine and nothing checks it until the
 * first spawn. spawnAgentCore then runs the install ladder (cliInstall.ts), and
 * for a provider with no `installCommand` and no `nativeInstallCommand` that
 * ladder ends at the `manual` rung: a hint is printed in a terminal and the
 * orchestrator never starts. This module turns the setup catalog probe
 * (`tools:status`, which already resolves every engine binary on PATH) into one
 * answer per provider so the wizard can say so BEFORE the pick is committed.
 *
 * The answer is informational: onboarding never blocks on it (the user may pick
 * any engine; missing ones are installed automatically where an installer
 * exists, and otherwise the user is told to install them).
 *
 * Pure and electron-free on purpose so it is testable from node --test.
 */
import type { AgentProvider } from './agentProvider';
import type { ToolStatus } from './toolCatalog';

export type EngineAvailabilityState =
  /** The binary resolves on this machine. */
  | 'installed'
  /** Not here yet, but the provider ships an installer the app runs on first spawn. */
  | 'installs-on-first-run'
  /** Not here and nothing we can run: the user has to install it by hand first. */
  | 'not-installable'
  /** The probe did not run or did not cover this provider. Never block on this. */
  | 'unknown';

export interface EngineAvailability {
  state: EngineAvailabilityState;
  /** Absolute path when installed. */
  path: string | null;
  /** The command the app would run (or the user could paste) to install it. */
  installCommand: string;
  docsUrl?: string;
}

/** Classify one provider from the catalog probe. `statuses` is what
 *  `window.cth.toolsStatus()` returned; undefined means it has not run (yet). */
export function classifyEngineAvailability(
  statuses: readonly ToolStatus[] | undefined,
  provider: AgentProvider
): EngineAvailability {
  const row = statuses?.find((s) => s.id === `engine:${provider}`);
  if (!row) return { state: 'unknown', path: null, installCommand: '' };
  const installCommand = row.installCommand ?? '';
  if (row.found) return { state: 'installed', path: row.path, installCommand, docsUrl: row.docsUrl };
  return {
    state: installCommand ? 'installs-on-first-run' : 'not-installable',
    path: null,
    installCommand,
    docsUrl: row.docsUrl
  };
}

/** English fallback text for the badge on each engine row. The wizard renders
 *  the localized `engineAvailabilityBadgeKey` instead. */
export function engineAvailabilityBadge(a: EngineAvailability): string | null {
  switch (a.state) {
    case 'installed': return 'INSTALLED';
    case 'installs-on-first-run': return 'INSTALLS ON FIRST RUN';
    case 'not-installable': return 'NOT INSTALLED';
    default: return null;
  }
}

/** i18n key for the badge on each engine row (null = no badge). Picking ANY
 *  engine is allowed in onboarding: a missing engine with an installer is set up
 *  automatically on first start, and one without an installer only gets a
 *  heads-up (never a blocked Next button). */
export function engineAvailabilityBadgeKey(a: EngineAvailability): string | null {
  switch (a.state) {
    case 'installed': return 'onboarding.orchestrator.badgeInstalled';
    case 'installs-on-first-run': return 'onboarding.orchestrator.badgeInstallsOnFirstRun';
    case 'not-installable': return 'onboarding.orchestrator.badgeNotInstalled';
    default: return null;
  }
}

/** Which engine the onboarding step should PRESELECT for the orchestrator.
 *
 *  The wizard used to hard-code Claude Code — fine for a Claude user, wrong for
 *  someone who only has Hermes (or Codex…) installed: they had to notice and
 *  re-pick, and a default of an engine that is not on the machine is a first run
 *  that installs something they never asked for. Order:
 *   1. Claude Code, if it is installed (it stays the app's long-standing default);
 *   2. otherwise Hermes, if installed (the preferred non-Claude engine);
 *   3. otherwise the first other INSTALLED engine in picker order;
 *   4. otherwise Claude Code (nothing is installed, or the probe has not run —
 *      it is the engine whose installer the app runs on first spawn).
 *  `eligible` is the picker's own order, so a suggestion can never be an engine
 *  the user cannot select. */
export function suggestOrchestratorEngine(
  statuses: readonly ToolStatus[] | undefined,
  eligible: readonly AgentProvider[]
): AgentProvider {
  const installed = (p: AgentProvider) =>
    eligible.includes(p) && classifyEngineAvailability(statuses, p).state === 'installed';
  if (installed('claude')) return 'claude';
  if (installed('hermes')) return 'hermes';
  const other = eligible.find((p) => installed(p));
  return other ?? 'claude';
}
