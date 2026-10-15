/**
 * UI mode — how much of the app is shown.
 *
 *  - 'simple'   : the essentials only (see what the team is doing, answer
 *                 questions, give tasks). Plain-language labels, a one-step
 *                 Add Agent, and a short Settings with an "advanced" switch.
 *  - 'advanced' : everything, exactly as before.
 *
 * `config.uiMode` is the explicit choice. The onboarding wizard writes it when
 * it finishes (non-technical → 'simple', otherwise 'advanced') and Settings →
 * "Show advanced options" changes it. It is NEVER inferred from `audience`:
 * an install that predates this setting has no `uiMode`, stays on 'advanced',
 * and so sees no layout change after an update — even if its owner once
 * answered "not technical" in the old wizard, which only changed wording.
 */
export type UiMode = 'simple' | 'advanced';

export interface UiModeConfig {
  uiMode?: UiMode;
  audience?: 'technical' | 'non-technical' | string;
}

export function resolveUiMode(config: UiModeConfig | null | undefined): UiMode {
  return config?.uiMode === 'simple' ? 'simple' : 'advanced';
}

export const isSimpleMode = (config: UiModeConfig | null | undefined): boolean =>
  resolveUiMode(config) === 'simple';
