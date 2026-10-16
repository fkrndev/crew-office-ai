import { createContext } from 'react';
import { useStore } from '@/store/store';

/**
 * Small seams between the simple-mode chat (ConversationView) and the composer it
 * hosts, kept out of both files so neither imports the other.
 */

/** True inside ConversationView's chat: the chat itself shows messages waiting to
 *  be sent (as greyed bubbles), so the composer must not list them a second time. */
export const ChatShowsQueueContext = createContext(false);

/** The window event the composer listens to in order to take focus. */
export const FOCUS_COMPOSER_EVENT = 'cth:focus-composer';

/**
 * Put `text` into an agent's composer and focus it — it is NEVER sent: the person
 * can edit it first. A draft that already has text is kept, and the idea goes
 * below it (nobody loses what they were typing). Used by the starter ideas.
 */
export function fillComposer(agentId: string, text: string): void {
  const s = useStore.getState();
  const cur = s.drafts[agentId] ?? '';
  s.setDraft(agentId, cur.trim() ? `${cur.replace(/\s+$/, '')}\n\n${text}` : text);
  window.dispatchEvent(new CustomEvent(FOCUS_COMPOSER_EVENT, { detail: { agentId } }));
}
