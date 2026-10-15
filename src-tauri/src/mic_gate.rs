//! Microphone permission gate — port of upstream 8c9e3037 / a84efe56
//! (src/main/index.ts `micFeatureLive` + the permission handlers).
//!
//! The only permission constrained is microphone capture: it is allowed ONLY
//! while a mic feature is actually live — Free Flow dictation enabled, or a
//! Realtime voice session (`realtimeVoiceEnabled`, which the renderer flips on
//! in src/realtime/session.ts `setMicGate(true)` BEFORE calling getUserMedia
//! and off again at stop). With both off there is zero mic access at the
//! WebView layer. Deliberately NOT gated on OpenAI-key presence (that key is
//! shared with the CLI engines). Every other permission kind returns
//! `Default`, i.e. unchanged behaviour.
//!
//! `realtimeVoiceEnabled` is force-closed at startup (a84efe56): a crash or
//! reload mid-session skips the renderer's teardown and would otherwise leave
//! the gate persisted OPEN on the next boot with no live session.
//!
//! Persistence (verified in real WebView2): wry's handler only calls
//! `SetState(ALLOW/DENY)` and WebView2's `SavesInProfile` defaults to true, so
//! the answer was stored per origin and every later request was answered from
//! the profile without reaching `decide` - a first Deny stuck across restarts,
//! a first Allow left the mic reachable with every mic feature off. On Windows
//! the `plugin()` below therefore (a) adds a second `PermissionRequested`
//! handler (runs after wry's, on the same args) that sets `SavesInProfile=false`
//! for the microphone, so the gate is evaluated on EVERY request and nothing is
//! stored, and (b) resets any microphone decision an earlier build already
//! stored (`ICoreWebView2Profile4::SetPermissionState(.., DEFAULT)`, mic kind
//! only - localStorage and every other permission are untouched). Other
//! platforms (WebKit) keep the plain handler: not verified there.

use crate::lock_ext::LockExt;
use tauri::plugin::TauriPlugin;
use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{Manager, Runtime, Webview};

/// Persist-kv key cmd_freeflow.rs stores the Free Flow switch under.
const KV_FREEFLOW_ENABLED: &str = "freeflow_enabled";

/// Is any mic feature live? Free Flow is read from both places it can live (the
/// persist kv this port uses, and the upstream-style `freeflowEnabled` config
/// key); the voice session from `realtimeVoiceEnabled`.
pub fn mic_feature_live(freeflow_kv: bool, freeflow_cfg: bool, realtime_voice: bool) -> bool {
    freeflow_kv || freeflow_cfg || realtime_voice
}

/// The whole policy: microphone follows the gate, everything else is Default.
pub fn decide(kind: PermissionKind, mic_live: bool) -> PermissionResponse {
    match kind {
        PermissionKind::Microphone => {
            if mic_live {
                PermissionResponse::Allow
            } else {
                PermissionResponse::Deny
            }
        }
        _ => PermissionResponse::Default,
    }
}

/// Read the live flags. Before AppState exists (a request during boot) the gate
/// is closed.
fn mic_live_now<R: Runtime>(app: &impl Manager<R>) -> bool {
    let Some(st) = app.try_state::<crate::commands::AppState>() else {
        return false;
    };
    let freeflow_kv = st
        .persist
        .lock_safe()
        .get_kv(KV_FREEFLOW_ENABLED)
        .and_then(|v| v.as_bool())
        == Some(true);
    let cfg = st.config_store.raw_value();
    let flag = |k: &str| cfg.get(k).and_then(|v| v.as_bool()) == Some(true);
    mic_feature_live(freeflow_kv, flag("freeflowEnabled"), flag("realtimeVoiceEnabled"))
}

/// `tauri::Builder::on_permission_request` handler.
pub fn on_permission_request<R: Runtime>(
    webview: Webview<R>,
    kind: PermissionKind,
) -> PermissionResponse {
    // This runs inside a WebView2 COM callback: a panic unwinding out of it
    // would abort the whole process, so a panic degrades to the safe answer
    // (mic denied, every other kind untouched).
    crate::panic_guard::guarded("permission-request", || {
        // Only look at the flags for the one kind we gate.
        let live = matches!(kind, PermissionKind::Microphone) && mic_live_now(&webview);
        decide(kind, live)
    })
    .unwrap_or(if matches!(kind, PermissionKind::Microphone) {
        PermissionResponse::Deny
    } else {
        PermissionResponse::Default
    })
}

/// Plugin that hardens EVERY webview as it is created (main, floor-*, anything
/// opened later) so the gate sees each microphone request - see the module doc.
/// A no-op off Windows.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("mic-gate")
        .on_webview_ready(|_webview| {
            #[cfg(windows)]
            if let Err(e) = _webview.with_webview(win::harden) {
                log::warn!("[mic-gate] could not reach the platform webview: {e}");
            }
        })
        .build()
}

/// WebView2 side of the gate: never persist the mic decision, and clear any
/// that is already stored.
#[cfg(windows)]
mod win {
    use tauri::webview::PlatformWebview;
    use webview2_com::Microsoft::Web::WebView2::Win32::*;
    use webview2_com::{
        take_pwstr, GetNonDefaultPermissionSettingsCompletedHandler,
        PermissionRequestedEventHandler, SetPermissionStateCompletedHandler,
    };
    use windows_core::{Interface, HSTRING, PWSTR};

    /// Runs on the main thread (`Webview::with_webview`). Failures are logged and
    /// leave the plain wry handler in charge (gate still enforced for the first
    /// request, just remembered by WebView2 as before).
    pub fn harden(w: PlatformWebview) {
        if let Err(e) = unsafe { harden_inner(&w) } {
            log::warn!("[mic-gate] WebView2 hardening failed: {e}");
        }
    }

    unsafe fn harden_inner(w: &PlatformWebview) -> windows_core::Result<()> {
        let core = w.controller().CoreWebView2()?;

        // (a) Registered after wry's handler, so it sees the same args once the
        // gate's Allow/Deny is set: flag the microphone answer as session-only.
        let mut token = 0i64;
        core.add_PermissionRequested(
            &PermissionRequestedEventHandler::create(Box::new(|_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                args.PermissionKind(&mut kind)?;
                if kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                    // Needs WebView2 runtime 1.0.1293+ (EventArgs3); older ones
                    // keep persisting - nothing more to do there.
                    if let Ok(a3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                        a3.SetSavesInProfile(false)?;
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;

        // (b) Forget a microphone decision an earlier build stored.
        let profile: ICoreWebView2Profile4 = core.cast::<ICoreWebView2_13>()?.Profile()?.cast()?;
        let p = profile.clone();
        profile.GetNonDefaultPermissionSettings(&GetNonDefaultPermissionSettingsCompletedHandler::create(
            Box::new(move |res, settings| {
                res?;
                let Some(settings) = settings else { return Ok(()) };
                let mut count = 0u32;
                settings.Count(&mut count)?;
                for i in 0..count {
                    let setting = settings.GetValueAtIndex(i)?;
                    let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                    setting.PermissionKind(&mut kind)?;
                    if kind != COREWEBVIEW2_PERMISSION_KIND_MICROPHONE {
                        continue;
                    }
                    let mut origin = PWSTR::null();
                    setting.PermissionOrigin(&mut origin)?;
                    let origin = HSTRING::from(take_pwstr(origin));
                    log::info!("[mic-gate] clearing a stored microphone decision for {origin}");
                    p.SetPermissionState(
                        kind,
                        &origin,
                        COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
                        &SetPermissionStateCompletedHandler::create(Box::new(|res| res)),
                    )?;
                }
                Ok(())
            }),
        ))?;
        Ok(())
    }
}

/// Startup hygiene: close `realtimeVoiceEnabled` if a previous run left it on.
/// Conditional write so a normal boot touches nothing. Returns whether it reset.
pub fn reset_realtime_voice_flag<R: Runtime>(app: &impl Manager<R>) -> bool {
    let Some(st) = app.try_state::<crate::commands::AppState>() else {
        return false;
    };
    let on = st
        .config_store
        .raw_value()
        .get("realtimeVoiceEnabled")
        .and_then(|v| v.as_bool())
        == Some(true);
    if !on {
        return false;
    }
    let mut patch = serde_json::Map::new();
    patch.insert("realtimeVoiceEnabled".into(), serde_json::json!(false));
    match st.config_store.patch_raw(&patch) {
        Ok(cfg) => {
            *st.config.lock_safe() = cfg;
            true
        }
        Err(e) => {
            log::warn!("[mic-gate] could not reset realtimeVoiceEnabled: {e:#}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microphone_is_denied_unless_a_mic_feature_is_live() {
        assert_eq!(decide(PermissionKind::Microphone, false), PermissionResponse::Deny);
        assert_eq!(decide(PermissionKind::Microphone, true), PermissionResponse::Allow);
    }

    #[test]
    fn every_other_kind_keeps_the_default_behaviour_whatever_the_gate_says() {
        for live in [false, true] {
            for kind in [
                PermissionKind::Camera,
                PermissionKind::Geolocation,
                PermissionKind::Notifications,
                PermissionKind::ClipboardRead,
                PermissionKind::DisplayCapture,
                PermissionKind::Midi,
                PermissionKind::Sensors,
                PermissionKind::Autoplay,
                PermissionKind::Other,
            ] {
                assert_eq!(decide(kind, live), PermissionResponse::Default, "{kind:?}");
            }
        }
    }

    #[test]
    fn gate_is_open_for_free_flow_or_a_voice_session_only() {
        assert!(!mic_feature_live(false, false, false));
        assert!(mic_feature_live(true, false, false));
        assert!(mic_feature_live(false, true, false));
        assert!(mic_feature_live(false, false, true));
        assert!(mic_feature_live(true, false, true));
    }

    #[test]
    fn live_flags_and_the_startup_reset_run_against_real_app_state() {
        let app = tauri::test::mock_app();
        let handle = app.app_handle().clone();
        let home = std::env::temp_dir().join(format!("oa-mic-{}", uuid::Uuid::new_v4()));
        handle.manage(crate::commands::AppState::new(home.clone()).unwrap());
        let st = handle.state::<crate::commands::AppState>();

        // fresh: closed
        assert!(!mic_live_now(&handle));
        assert!(!reset_realtime_voice_flag(&handle), "a clean boot writes nothing");

        // a voice session opens the gate (the renderer persists the flag) ...
        let mut p = serde_json::Map::new();
        p.insert("realtimeVoiceEnabled".into(), serde_json::json!(true));
        st.config_store.patch_raw(&p).unwrap();
        assert!(mic_live_now(&handle));
        // ... and a crash left it open: startup closes it again
        assert!(reset_realtime_voice_flag(&handle));
        assert!(!mic_live_now(&handle));
        assert_eq!(
            st.config_store.raw_value().get("realtimeVoiceEnabled"),
            Some(&serde_json::json!(false))
        );

        // Free Flow (persist kv, this port's home for the switch) opens it too
        st.persist.lock_safe().set_kv(KV_FREEFLOW_ENABLED, &serde_json::json!(true));
        assert!(mic_live_now(&handle));
        st.persist.lock_safe().set_kv(KV_FREEFLOW_ENABLED, &serde_json::json!(false));
        assert!(!mic_live_now(&handle));
        // ... and so does the upstream-style config key
        let mut p = serde_json::Map::new();
        p.insert("freeflowEnabled".into(), serde_json::json!(true));
        st.config_store.patch_raw(&p).unwrap();
        assert!(mic_live_now(&handle));
        // the startup reset only ever touches the voice flag
        assert!(!reset_realtime_voice_flag(&handle));
        assert!(mic_live_now(&handle));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn no_app_state_means_a_closed_gate_and_no_reset() {
        // A mock app with no AppState managed (boot-time request): closed.
        let app = tauri::test::mock_app();
        assert!(!mic_live_now(app.handle()));
        assert!(!reset_realtime_voice_flag(app.handle()));
    }
}
