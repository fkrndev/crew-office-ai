/**
 * Audience register — the "who are you" answer from onboarding (and the
 * Settings → "Explain things simply" toggle) decides how the orchestrator
 * talks to the human. The flag is stored as `config.audience`; this module
 * turns it into the instruction god actually receives, so the setting is more
 * than a label.
 *
 * Shared between the boot prompt (fresh god spawn) and the live-change notice
 * (toggled while god is running — a resumed god never sees the boot prompt).
 */
export type Audience = 'technical' | 'non-technical';

/** Instruction for a non-technical human. Scoped to what is written TO the
 *  human so delegation between agents stays precise. */
const PLAIN_BRIEF =
  'The human you work for is not technical. Whenever you write TO THE HUMAN (status updates, questions, final reports, Ask Me cards), use plain everyday language: say what happened and what you need from them in one or two short sentences, avoid engineering jargon (worktree, PTY, hook, breaker, token, commit hash, …) or explain it in a few words, and give technical detail only if they ask. Messages to other agents stay as technical as the work needs. When you brief or hire workers, tell them the same rule for anything they send the human directly.';

/** Instruction for a technical human (also the switch-back message). */
const TECHNICAL_BRIEF =
  'The human you work for is technical. You may use precise engineering terms and include concrete detail (file paths, commands, diffs) when you write to them; keep it concise.';

/** The line appended to god's first prompt, or null when the default register
 *  needs no extra instruction. */
export function audienceBrief(audience: Audience | undefined): string | null {
  return audience === 'non-technical' ? PLAIN_BRIEF : null;
}

/** The hive message sent to a RUNNING god when the setting changes. */
export function audienceChangeMessage(audience: Audience | undefined): {
  subject: string;
  body: string;
} {
  return {
    subject: 'PREFERENCE: how to talk to the human',
    body: `${audience === 'non-technical' ? PLAIN_BRIEF : TECHNICAL_BRIEF}\n\nThe human just changed this setting — apply it from your next message to them onward. No reply needed.`,
  };
}
