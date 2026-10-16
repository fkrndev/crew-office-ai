import type { FriendlyContext } from './friendlyError';

/**
 * The `cth:notice` banner: failures that happen deep in the tree with no UI of
 * their own surface through one dismissible banner in App.
 *
 *   text        what advanced mode shows (unchanged wording);
 *   simpleText  a plain-language replacement for simple mode, when the sentence
 *               itself is technical (and there is no raw error to map);
 *   raw         the underlying error — simple mode maps it to a friendly title
 *               and hint (shared/friendlyError.ts) and keeps `raw` behind a
 *               "Details" disclosure;
 *   context     which kind of action failed (better headline for unknown errors).
 */
export interface NoticeDetail {
  text: string;
  simpleText?: string;
  raw?: unknown;
  context?: FriendlyContext;
}

export const NOTICE_EVENT = 'cth:notice';

export function emitNotice(detail: NoticeDetail): void {
  window.dispatchEvent(new CustomEvent<NoticeDetail>(NOTICE_EVENT, { detail }));
}

/** Read a `cth:notice` event's payload defensively. */
export function noticeFromEvent(e: Event): NoticeDetail | null {
  const d = (e as CustomEvent<Partial<NoticeDetail> | undefined>).detail;
  if (!d || typeof d.text !== 'string' || !d.text) return null;
  return { text: d.text, simpleText: typeof d.simpleText === 'string' ? d.simpleText : undefined, raw: d.raw, context: d.context };
}
