/**
 * Auto-update toast (v0.3.4) — the visual half of the background updater.
 *
 * Main's updater (cmd_update.rs) checks the signed feed and pushes these over
 * `update:status`:
 *   - 'available'         → a SIGNED update exists; offer "download update"
 *                           (then 'downloading' progress, then 'downloaded').
 *   - 'downloaded'        → the update is staged; offer "restart to update".
 *   - 'available-manual'  → no signed artifact (or its download failed); offer
 *                           a link to the release page.
 *
 * Mirrors CompletionToast: self-contained + self-subscribing, mounted once in
 * App.tsx, renders nothing when idle. Installation is ALWAYS user-initiated —
 * "later" just hides the toast until the next app start (or the 6h re-check).
 *
 * ─── v0.4.4: "What's new" ───────────────────────────────────────────────────
 * Both states already carried `notes` (the GitHub release body) and the toast
 * dropped it on the floor, so the only notification this app ever raises said
 * nothing but a version number. It now renders a digest of that body —
 * summarizeReleaseNotes() in src/shared/releaseNotes.ts does the parsing, and
 * lives there rather than here so it can be unit-tested without a renderer.
 *
 * Three rules this block obeys:
 *   1. No notes, no block. A release body that is missing, empty, or pure
 *      structure yields an empty digest and the toast renders EXACTLY as it did
 *      before — no orphan heading, no shifted buttons. Most bodies are like
 *      that, so this is the common path, not the edge case.
 *   2. Bounded height. The digest is capped in releaseNotes.ts AND clamped with
 *      a scroll here, because a toast that grows with the release notes is a
 *      dialog that covers the app.
 *   3. The star ask is shown AT MOST ONCE EVER, not once per release. A repeated
 *      ask is the kind of nagging that gets a notification muted, which would
 *      cost the updater its only channel. See STAR_ASK_KEY below.
 *
 * No new IPC and no new network call: "read more" reuses `updateOpenRelease`
 * (the same bridge the manual state's button has always used) and the star link
 * goes through the existing `openExternal` opener.
 */
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Icon } from '@/components/Icon';
import { summarizeReleaseNotes } from '@shared/releaseNotes';
import { extractDropHtml } from '@shared/releaseDrop';
import { ReleaseDrop } from '@/components/ReleaseDrop';
import { clampPercent, type UpdateStatus } from '@shared/updateState';

/** The toast is the LOUD half — it only interrupts for the states a user has
 *  to act on: a signed update to download (`available`), its progress
 *  (`downloading`, only while a toast is already up), a staged one to restart
 *  into (`downloaded`), or the notify-only manual path. Checks and errors live
 *  quietly in the toolbar badge next to the logo. */
type ToastStatus = Extract<
  UpdateStatus,
  { state: 'available' | 'downloading' | 'downloaded' | 'available-manual' | 'just-updated' }
>;

function toastable(s: UpdateStatus): ToastStatus | null {
  return s.state === 'available' || s.state === 'downloading' || s.state === 'downloaded'
    || s.state === 'available-manual' || s.state === 'just-updated' ? s : null;
}

const GITHUB_REPO_URL = 'https://github.com/fkrndev/crew-office-ai';
/** Only ever the `href` — the click is handled by `updateOpenRelease`, which
 *  resolves `undefined` to this same page in main. */
const GITHUB_RELEASES_URL = `${GITHUB_REPO_URL}/releases/latest`;

export function UpdateToast() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<ToastStatus | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => window.cth.onUpdateStatus?.((next) => {
    const t = toastable(next);
    // A non-toastable state (a re-check, say) must not erase a toast the user
    // hasn't answered yet — only a new actionable state replaces it.
    if (!t) return;
    // Progress ticks only update a toast that is already up: after "later" a
    // download keeps ticking and must not pop the toast back at every percent.
    if (t.state === 'downloading') { setStatus((prev) => (prev ? t : prev)); return; }
    setStatus(t);
  }), []);

  // Main may have emitted before this window existed (a downloaded update
  // from a previous session, or the dev-only MD_DROP_PREVIEW boot hook), and a
  // push nobody was listening to is gone. Pull the last status once on mount so
  // that state is not lost.
  useEffect(() => {
    let alive = true;
    void window.cth.updateCurrent?.().then((cur) => {
      const t = toastable(cur);
      if (alive && t) setStatus((prev) => prev ?? t);
    }).catch(() => { /* nothing to show */ });
    return () => { alive = false; };
  }, []);

  // Settings' hero card asks to re-open the release notes. This surface owns the
  // last status and the drop renderer, so it answers rather than duplicating
  // either. `updateCurrent()` is used instead of the remembered state because
  // "later" clears the local copy while main still holds it — dismissing a
  // release must not make it unreadable afterwards. With genuinely nothing to
  // show (a dev build, or an install already on the newest release) the honest
  // answer is the releases page, not an empty modal.
  useEffect(() => {
    const onShow = async () => {
      try {
        const cur = await window.cth.updateCurrent();
        const t = toastable(cur);
        if (t) { setStatus(t); return; }
      } catch { /* fall through to the page */ }
      void window.cth.updateOpenRelease();
    };
    window.addEventListener('cth:show-release-notes', onShow);
    return () => window.removeEventListener('cth:show-release-notes', onShow);
  }, []);

  const notes = useMemo(() => summarizeReleaseNotes(status?.notes), [status?.notes]);
  /** An authored <!-- drop --> block in the release body upgrades this whole
   *  moment from a corner toast to a centered release page. Absent (every
   *  release published so far), everything below behaves exactly as before —
   *  the digest path stays the default, not a fallback nobody exercises. */
  const dropHtml = useMemo(() => extractDropHtml(status?.notes), [status?.notes]);
  const version = status?.version ?? null;

  if (!status) return null;

  /** Close the notice FIRST, then ask main to quit and install. The quit path
   *  raises the kill-and-quit warning when agents are running, and leaving a
   *  "restarting…" notice on screen behind it just gives the user two things to
   *  read. (The warning outranking every modal is a separate fix — this one is
   *  about not asking two questions at once.) If main reports it could not quit,
   *  the notice comes back so the user can retry; a user CANCEL of the warning
   *  is not a failure, and the notice stays closed. */
  const restart = async () => {
    const prev = status;
    setBusy(true);
    setStatus(null);
    try {
      const res = await window.cth.updateRestartAndInstall();
      if (!res.ok) { setStatus(prev); setBusy(false); }
    } catch { setStatus(prev); setBusy(false); }
  };

  /** Same call the manual state's button makes: main resolves `undefined` to
   *  the releases page and refuses any URL outside this repo. */
  const openRelease = () => {
    void window.cth.updateOpenRelease(
      status.state === 'available-manual' ? (status.downloadUrl ?? status.url) : undefined
    );
  };
  /** True when the release carries an installer for THIS machine, so the button
   *  can promise a download rather than a page to go hunting on. */
  const hasDownload = status.state === 'available-manual' && !!status.downloadUrl;

  /** Start the signed download. Main answers `manual-download` when it holds no
   *  signed artifact (the notify-only fallback): then the release page is the
   *  path, exactly as before. Progress arrives as `downloading` events. */
  const download = async () => {
    setBusy(true);
    try {
      const res = await window.cth.updateDownload();
      if (!res?.ok) openRelease();
    } catch { openRelease(); }
    setBusy(false);
  };

  // An authored release: hand the whole moment to the centered drop instead of
  // the corner toast. Nothing is passed in but the content — the drop carries no
  // app buttons, and its own links go out through the OS browser.
  //
  // Restart-to-install is NOT lost with the button: autoInstallOnAppQuit is off,
  // so the update needs an explicit restart, and the title-bar UpdateBadge (and
  // Settings -> Updates) still offer it after this is dismissed.
  if (dropHtml && version) {
    return (
      <ReleaseDrop
        version={version}
        html={dropHtml}
        onDismiss={() => setStatus(null)}
      />
    );
  }
  // Freshly updated with nothing authored for this release: nothing to say.
  if (status.state === 'just-updated') return null;

  const buttonStyle: React.CSSProperties = {
    padding: '3px 10px 1px',
    background: 'var(--cth-mint-light, #d0f0e0)',
    boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)',
    fontFamily: 'var(--cth-font-ui)', fontSize: 12,
    color: 'var(--cth-ink-900)', cursor: 'pointer', border: 'none'
  };

  const linkStyle: React.CSSProperties = {
    fontSize: 12, lineHeight: '16px', color: 'var(--cth-ink-900)',
    textDecoration: 'underline', cursor: 'pointer'
  };

  return (
    <div style={{
      position: 'fixed', right: 16, bottom: 16, zIndex: 400,
      maxWidth: 340,
      background: 'var(--cth-cream-50)',
      boxShadow: '0 0 0 2px var(--cth-ink-900), 4px 5px 0 0 rgba(15,28,46,0.25)',
      padding: '10px 12px',
      display: 'flex', flexDirection: 'column', gap: 8,
      fontFamily: 'var(--cth-font-ui)'
    }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <Icon name="sparkle" />
        <span style={{ fontSize: 13, color: 'var(--cth-ink-900)', fontWeight: 600 }}>
          {status.state === 'downloaded'
            ? t('updateToast.downloaded', { version: status.version })
            : status.state === 'downloading'
              ? t('updateToast.downloading', { version: status.version, percent: clampPercent(status.percent) })
              : t('updateToast.available', { version: status.version })}
        </span>
      </div>
      <span style={{ fontSize: 12, lineHeight: '16px', color: 'var(--cth-ink-700)' }}>
        {status.state === 'downloaded'
          ? t('updateToast.downloadedBody')
          : status.state === 'available'
            ? t('updateToast.availableBody')
            : status.state === 'downloading'
              ? t('updateToast.downloadingBody')
              : status.state === 'available-manual' && status.reason
                ? t('updateToast.manualBodyReason', { reason: status.reason })
                : t('updateToast.manualBody')}
      </span>
      {status.state === 'downloading' && (
        <div
          role="progressbar"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={clampPercent(status.percent)}
          style={{ height: 8, background: 'var(--cth-cream-100)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-300)' }}
        >
          <div style={{
            height: '100%', width: `${clampPercent(status.percent)}%`,
            background: 'var(--cth-mint-light, #d0f0e0)', boxShadow: 'inset 0 0 0 1px var(--cth-ink-900)',
            transition: 'width 120ms linear'
          }} />
        </div>
      )}

      {notes.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          <div style={{
            fontFamily: 'var(--cth-font-display)', fontSize: 8, lineHeight: '12px',
            color: 'var(--cth-ink-500)', textTransform: 'uppercase'
          }}>
            {t('updateToast.whatsNew')}
          </div>
          {/* The digest is already capped at ~280 chars; the clamp is the second
              belt, for the day a release body defeats the parser. */}
          <ul style={{
            listStyle: 'none', margin: 0, padding: '0 0 0 2px',
            maxHeight: 96, overflowY: 'auto',
            display: 'flex', flexDirection: 'column', gap: 4
          }}>
            {notes.map((line, i) => (
              <li key={i} style={{
                display: 'flex', gap: 6,
                fontSize: 12, lineHeight: '16px', color: 'var(--cth-ink-700)'
              }}>
                <span aria-hidden style={{ color: 'var(--cth-ink-300)' }}>•</span>
                <span>{line}</span>
              </li>
            ))}
          </ul>
          <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap' }}>
            <a
              href={status.state === 'available-manual' || status.state === 'available' ? (status.url ?? GITHUB_RELEASES_URL) : GITHUB_RELEASES_URL}
              onClick={(e) => { e.preventDefault(); openRelease(); }}
              style={linkStyle}
            >{t('updateToast.readMore')}</a>
          </div>
        </div>
      )}

      <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
        <button
          onClick={() => setStatus(null)}
          style={{ ...buttonStyle, background: 'var(--cth-cream-100)' }}
        >
          {t('updateToast.later')}
        </button>
        {status.state === 'downloaded' ? (
          <button onClick={restart} disabled={busy} style={buttonStyle}>
            {busy ? t('updateToast.restarting') : t('updateToast.restartToUpdate')}
          </button>
        ) : status.state === 'available' ? (
          <button onClick={() => { void download(); }} disabled={busy} style={buttonStyle}>
            {t('updateToast.downloadUpdate', { version: status.version })}
          </button>
        ) : status.state === 'downloading' ? (
          <button disabled style={{ ...buttonStyle, cursor: 'default', opacity: 0.7 }}>
            {t('updateToast.downloadingShort')}
          </button>
        ) : (
          <button
            onClick={openRelease}
            style={buttonStyle}
          >
            {hasDownload ? t('updateToast.download', { version: status.version }) : t('updateToast.openReleases')}
          </button>
        )}
      </div>
    </div>
  );
}
