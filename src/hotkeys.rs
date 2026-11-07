//! Global hotkeys: configurable OS-wide shortcuts that dispatch the
//! same [`Command`]s as the UI — `RegisterHotKey` on Windows, Carbon
//! `RegisterEventHotKey` on macOS, both via `global-hotkey`.
//!
//! Two pieces: [`HotkeyManager`] owns the registrations and keeps an
//! `action → HotKey` map plus an id→action index; [`listen`] parks a
//! thread on `GlobalHotKeyEvent::receiver()` which pushes the mapped
//! command and calls [`wake_viewports`] — egui does not repaint while
//! every window is hidden, which is precisely when these fire.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::app::{Command, Shared, lock, wake_viewports};

/// The actions a global hotkey can fire — one `Command` each. Adding a
/// variant extends the settings page and the dispatch automatically.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum HotkeyAction {
    /// Show/raise the viewer (the resident menu's "Open PixMe" — never
    /// a toggle, never a second window).
    OpenViewer,
    /// Start a rectangular-region capture on the viewer's monitor.
    CaptureRegion,
}

impl HotkeyAction {
    /// Every bindable action, in settings-page order.
    pub const ALL: [Self; 2] = [Self::OpenViewer, Self::CaptureRegion];

    /// Pushes the action's command through the shared machine —
    /// `CaptureRegion` rides `push_capture` so the start monitor is
    /// recorded exactly like a capture-bar click.
    pub fn dispatch(&self, shared: &Shared, ctx: &egui::Context) {
        match self {
            Self::OpenViewer => shared.push(Command::OpenViewer),
            Self::CaptureRegion => shared.push_capture(crate::app::CaptureMode::Region, ctx),
        }
    }
}

/// Why a combo cannot be armed — surfaced as the settings badge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    /// Another application (or the OS) already owns the combo — the
    /// definitive check happens at registration time since neither OS
    /// can enumerate foreign hotkeys.
    Taken,
    /// A reserved OS shortcut (Cmd+Shift+3/4/5 on macOS, Win+L /
    /// Ctrl+Alt+Del on Windows).
    Reserved,
}

/// `true` when `hk` collides with a shortcut the OS reserves —
/// rejected locally before wasting a registration attempt.
#[cfg(target_os = "macos")]
pub fn is_reserved(hk: &HotKey) -> bool {
    let cmd_shift = Modifiers::META | Modifiers::SHIFT;
    if hk.mods == cmd_shift && matches!(hk.key, Code::Digit3 | Code::Digit4 | Code::Digit5) {
        return true; // system screenshot shortcuts — never reused
    }
    hk.mods == Modifiers::META
        && matches!(
            hk.key,
            Code::Space | Code::Tab | Code::KeyQ | Code::KeyW | Code::KeyH | Code::KeyM
        )
}

/// `true` when `hk` collides with a shortcut the OS reserves (see above).
#[cfg(target_os = "windows")]
pub fn is_reserved(hk: &HotKey) -> bool {
    (hk.mods == Modifiers::META && matches!(hk.key, Code::KeyL | Code::KeyD | Code::KeyE))
        || (hk.mods == (Modifiers::CONTROL | Modifiers::ALT)
            && matches!(hk.key, Code::Delete | Code::Escape))
}

/// Posts `hk` as a real HID key event — the settings "Test" button's
/// probe. Whoever owns the combo receives it: our own Carbon
/// registration (→ consumed by the listener as the test answer) or a
/// foreign app (→ our listen window times out). `CGEventPost` needs
/// no Accessibility permission (unlike event taps). Returns `false`
/// for unmapped keys.
#[cfg(target_os = "macos")]
pub fn synthesize(hk: &HotKey) -> bool {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    let Some(code) = code_to_vk(hk.key) else {
        return false;
    };
    let mut flags = CGEventFlags::empty();
    if hk.mods.contains(Modifiers::CONTROL) {
        flags |= CGEventFlags::CGEventFlagControl;
    }
    if hk.mods.contains(Modifiers::ALT) {
        flags |= CGEventFlags::CGEventFlagAlternate;
    }
    if hk.mods.contains(Modifiers::SHIFT) {
        flags |= CGEventFlags::CGEventFlagShift;
    }
    if hk.mods.contains(Modifiers::META) {
        flags |= CGEventFlags::CGEventFlagCommand;
    }
    let Ok(src) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
        return false;
    };
    for down in [true, false] {
        let Ok(ev) = CGEvent::new_keyboard_event(src.clone(), code, down) else {
            return false;
        };
        ev.set_flags(flags);
        ev.post(CGEventTapLocation::HID);
    }
    true
}

/// `Code` → macOS virtual keycode (kVK_*), for [`synthesize`].
#[cfg(target_os = "macos")]
fn code_to_vk(code: Code) -> Option<u16> {
    Some(match code {
        Code::KeyA => 0x00,
        Code::KeyS => 0x01,
        Code::KeyD => 0x02,
        Code::KeyF => 0x03,
        Code::KeyH => 0x04,
        Code::KeyG => 0x05,
        Code::KeyZ => 0x06,
        Code::KeyX => 0x07,
        Code::KeyC => 0x08,
        Code::KeyV => 0x09,
        Code::KeyB => 0x0B,
        Code::KeyQ => 0x0C,
        Code::KeyW => 0x0D,
        Code::KeyE => 0x0E,
        Code::KeyR => 0x0F,
        Code::KeyY => 0x10,
        Code::KeyT => 0x11,
        Code::Digit1 => 0x12,
        Code::Digit2 => 0x13,
        Code::Digit3 => 0x14,
        Code::Digit4 => 0x15,
        Code::Digit6 => 0x16,
        Code::Digit5 => 0x17,
        Code::Equal => 0x18,
        Code::Digit9 => 0x19,
        Code::Digit7 => 0x1A,
        Code::Minus => 0x1B,
        Code::Digit8 => 0x1C,
        Code::Digit0 => 0x1D,
        Code::BracketRight => 0x1E,
        Code::KeyO => 0x1F,
        Code::KeyU => 0x20,
        Code::BracketLeft => 0x21,
        Code::KeyI => 0x22,
        Code::KeyP => 0x23,
        Code::Enter => 0x24,
        Code::KeyL => 0x25,
        Code::KeyJ => 0x26,
        Code::Quote => 0x27,
        Code::KeyK => 0x28,
        Code::Semicolon => 0x29,
        Code::Backslash => 0x2A,
        Code::Comma => 0x2B,
        Code::Slash => 0x2C,
        Code::KeyN => 0x2D,
        Code::KeyM => 0x2E,
        Code::Period => 0x2F,
        Code::Tab => 0x30,
        Code::Space => 0x31,
        Code::Backquote => 0x32,
        Code::Backspace => 0x33,
        Code::Escape => 0x35,
        Code::F5 => 0x60,
        Code::F6 => 0x61,
        Code::F7 => 0x62,
        Code::F3 => 0x63,
        Code::F8 => 0x64,
        Code::F9 => 0x65,
        Code::F11 => 0x67,
        Code::F10 => 0x6D,
        Code::F12 => 0x6F,
        Code::Insert => 0x72,
        Code::Home => 0x73,
        Code::PageUp => 0x74,
        Code::Delete => 0x75,
        Code::F4 => 0x76,
        Code::End => 0x77,
        Code::F2 => 0x78,
        Code::PageDown => 0x79,
        Code::F1 => 0x7A,
        Code::ArrowLeft => 0x7B,
        Code::ArrowRight => 0x7C,
        Code::ArrowDown => 0x7D,
        Code::ArrowUp => 0x7E,
        _ => return None,
    })
}

/// Owned registrations: armed combos, the id→action index the listener
/// thread reads, and the OS manager. Dropping unregisters everything.
pub struct HotkeyManager {
    manager: GlobalHotKeyManager,
    armed: BTreeMap<HotkeyAction, HotKey>,
    ids: Arc<Mutex<BTreeMap<u32, HotkeyAction>>>,
    /// `true` while the settings recorder listens: every armed combo is
    /// unregistered so a re-recorded key press reaches the app instead
    /// of being consumed by its own hotkey (and firing its action).
    suspended: bool,
    /// Combo registered for the settings confirmation press, with its
    /// action — transient, separate from `armed` (the settings apply
    /// adds it to the armed set on OK). The confirm press must reach
    /// the listener before the binding is trusted; a combo not yet in
    /// `armed` cannot fire otherwise.
    verify: Option<(HotKey, HotkeyAction)>,
}

impl HotkeyManager {
    pub fn new() -> global_hotkey::Result<Self> {
        Ok(Self {
            manager: GlobalHotKeyManager::new()?,
            armed: BTreeMap::new(),
            ids: Arc::new(Mutex::new(BTreeMap::new())),
            suspended: false,
            verify: None,
        })
    }

    /// The shared id→action index handed to [`listen`].
    pub fn ids(&self) -> Arc<Mutex<BTreeMap<u32, HotkeyAction>>> {
        self.ids.clone()
    }

    /// Unregisters every armed combo and clears the id index — used
    /// while a settings field is listening. Pressing an armed combo
    /// would otherwise be consumed by the OS hotkey: neither its key
    /// press would reach the recorder nor could it be re-recorded. The
    /// `armed` map is kept; [`resume`] re-registers the same set.
    pub fn suspend(&mut self) {
        if self.suspended {
            return;
        }
        for hk in self.armed.values() {
            let _ = self.manager.unregister(*hk);
        }
        self.ids.lock().unwrap().clear();
        self.suspended = true;
        tracing::debug!(count = self.armed.len(), "hotkeys suspended");
    }

    /// Re-registers the combos parked by [`suspend`] and rebuilds the
    /// id index. Idempotent; a combo the OS suddenly refuses is logged
    /// and skipped (the settings badge reflects it on the next sync).
    pub fn resume(&mut self) {
        if !self.suspended {
            return;
        }
        for (action, hk) in &self.armed {
            match self.manager.register(*hk) {
                Ok(()) => {
                    self.ids.lock().unwrap().insert(hk.id, *action);
                }
                Err(err) => {
                    tracing::warn!(action = ?action, raw = %hk, "hotkey resume failed: {err}");
                }
            }
        }
        self.suspended = false;
        tracing::debug!(count = self.armed.len(), "hotkeys resumed");
    }

    /// `true` between [`suspend`] and [`resume`] — the settings page
    /// uses it to re-arm on every exit path.
    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// `true` when `hk` is already armed for ANY action — a proven
    /// binding needs no confirmation press, and a transient verify
    /// registration on the same id would collide with the live one
    /// (the lib keeps a single ref per id: a second register orphans
    /// the first Carbon registration).
    pub fn is_armed(&self, hk: &HotKey) -> bool {
        self.armed.values().any(|a| a == hk)
    }

    /// Registers `hk` transiently so a test press can reach the
    /// listener — for combos that are NOT armed (an armed combo's
    /// existing registration already delivers its presses; the
    /// consume slot alone marks them). The registration lives until
    /// [`release_verify`]. An armed `hk` is refused outright: the lib
    /// keeps a single ref per id, so a second register would orphan
    /// the live Carbon registration and the release would kill it.
    pub fn register_verify(&mut self, hk: HotKey, action: HotkeyAction) {
        if self.verify.is_some() {
            self.release_verify();
        }
        if self.is_armed(&hk) {
            return;
        }
        match self.manager.register(hk) {
            Ok(()) => {
                self.ids.lock().unwrap().insert(hk.id, action);
                self.verify = Some((hk, action));
                tracing::debug!(action = ?action, raw = %hk, "hotkey verify armed");
            }
            Err(err) => {
                tracing::warn!(action = ?action, raw = %hk, "hotkey verify register failed: {err}");
            }
        }
    }

    /// Releases the transient test registration — called when the
    /// test press arrived, the window timed out, or the settings
    /// modal closed. Idempotent.
    pub fn release_verify(&mut self) {
        if let Some((hk, action)) = self.verify.take() {
            let _ = self.manager.unregister(hk);
            let mut ids = self.ids.lock().unwrap();
            // The id may have been re-registered for the same action
            // by a sync — only drop it if it still maps to us.
            if ids.get(&hk.id) == Some(&action) {
                ids.remove(&hk.id);
            }
            tracing::debug!(action = ?action, raw = %hk, "hotkey verify released");
        }
    }

    /// Registers `cfg`'s combos and unregisters whatever left the map
    /// or changed. A failed registration KEEPS the previous armed combo
    /// working — the old shortcut is only released once its replacement
    /// succeeded. Returns the per-action failures for the settings
    /// badge (`empty` = all good).
    pub fn sync(
        &mut self,
        cfg: &BTreeMap<HotkeyAction, String>,
    ) -> BTreeMap<HotkeyAction, HotkeyError> {
        let mut errors = BTreeMap::new();
        let mut wanted: BTreeMap<HotkeyAction, HotKey> = BTreeMap::new();
        for action in HotkeyAction::ALL {
            if let Some(raw) = cfg.get(&action)
                && let Ok(hk) = raw.parse::<HotKey>()
            {
                wanted.insert(action, hk);
            }
        }
        // A combo claimed by two actions arms neither new claimant.
        let mut taken_by: BTreeMap<u32, HotkeyAction> = BTreeMap::new();
        for (action, hk) in &wanted {
            if let Some(prev) = taken_by.insert(hk.id, *action) {
                errors.insert(*action, HotkeyError::Taken);
                errors.insert(prev, HotkeyError::Taken);
            }
        }
        // Release pass: unregister every armed combo whose binding
        // left or changed BEFORE the new ones are registered — the
        // lib keeps a single ref per hotkey id, so registering a new
        // owner of the same combo first would orphan the old Carbon
        // registration. `released` keeps each old combo for the
        // failure path below (a refused replacement restores it).
        let mut released: BTreeMap<HotkeyAction, HotKey> = BTreeMap::new();
        for action in HotkeyAction::ALL {
            if wanted.get(&action).copied() == self.armed.get(&action).copied() {
                continue;
            }
            if let Some(old) = self.armed.remove(&action) {
                released.insert(action, old);
                let _ = self.manager.unregister(old);
                self.ids.lock().unwrap().remove(&old.id);
            }
        }
        for action in HotkeyAction::ALL {
            let Some(hk) = wanted.get(&action).copied() else {
                if released.remove(&action).is_some() {
                    tracing::info!(action = ?action, "hotkey unregistered");
                }
                continue;
            };
            if self.armed.get(&action).copied() == Some(hk) {
                continue; // unchanged binding, still armed
            }
            // An OS-reserved combo joins the internal duplicates in
            // `errors` (a dup keeps its `Taken` — `or_insert` leaves
            // occupied slots alone); either way the new binding is
            // rejected and the released one restored below.
            let bad = if is_reserved(&hk) {
                errors.entry(action).or_insert(HotkeyError::Reserved);
                true
            } else {
                errors.contains_key(&action)
            };
            if !bad {
                match self.manager.register(hk) {
                    Ok(()) => {
                        self.armed.insert(action, hk);
                        self.ids.lock().unwrap().insert(hk.id, action);
                        tracing::info!(action = ?action, raw = %hk, "hotkey armed");
                        continue;
                    }
                    Err(err) => {
                        tracing::warn!(action = ?action, raw = %hk, "hotkey register failed: {err}");
                        errors.insert(action, HotkeyError::Taken);
                    }
                }
            }
            // The new binding was rejected — restore the released one
            // so the action keeps a working shortcut.
            if let Some(old) = released.remove(&action)
                && self.manager.register(old).is_ok()
            {
                self.armed.insert(action, old);
                self.ids.lock().unwrap().insert(old.id, action);
            }
        }
        errors
    }

    /// Whether the OS would accept `hk` right now — a register +
    /// immediate unregister on THIS manager, so the armed combos are
    /// never disturbed and the listener never fires (the id→action
    /// index is untouched by the probe). A dedicated throwaway
    /// manager cannot work: on macOS, installing the Carbon event
    /// handler a second time on the application target fails, so the
    /// probe must reuse the one manager the app owns. `false` also on
    /// parse/Reserved; `Taken` when no manager exists at all.
    pub fn probe(&self, raw: &str) -> Result<(), HotkeyError> {
        let Ok(hk) = raw.parse::<HotKey>() else {
            return Err(HotkeyError::Reserved);
        };
        if is_reserved(&hk) {
            return Err(HotkeyError::Reserved);
        }
        match self.manager.register(hk) {
            Ok(()) => {
                let _ = self.manager.unregister(hk);
                Ok(())
            }
            Err(err) => {
                tracing::debug!(raw, "hotkey probe failed: {err}");
                Err(HotkeyError::Taken)
            }
        }
    }
}

impl Drop for HotkeyManager {
    fn drop(&mut self) {
        if let Some((hk, _)) = self.verify.take() {
            let _ = self.manager.unregister(hk);
        }
        let _ = self
            .manager
            .unregister_all(&self.armed.values().copied().collect::<Vec<_>>());
    }
}

/// Parks a thread on the global event channel: every press pushes its
/// action's `Command` and wakes the UI — the egui loop does not repaint
/// while hidden, which is exactly when hotkeys land.
pub fn listen(
    ids: Arc<Mutex<BTreeMap<u32, HotkeyAction>>>,
    shared: Arc<Shared>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        let rx = GlobalHotKeyEvent::receiver();
        while let Ok(ev) = rx.recv() {
            if ev.state != HotKeyState::Pressed {
                continue;
            }
            tracing::debug!(id = ev.id, "hotkey event received");
            let action = ids.lock().unwrap().get(&ev.id).copied();
            if let Some(action) = action {
                // The settings page is testing a bound combo: the
                // press proves the combo is deliverable — consume it
                // WITHOUT dispatching (a capture must not start from
                // a test press) and clear the slot; the settings page
                // observes the clear. The event id must match — other
                // armed combos keep firing normally during the window.
                if lock(&shared.hotkey_verify).is_some_and(|v| v == ev.id) {
                    *lock(&shared.hotkey_verify) = None;
                    continue;
                }
                action.dispatch(&shared, &ctx);
                wake_viewports(&ctx);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `probe` on a fresh manager accepts a free combo and reports
    /// `Taken` once the same combo is armed by that manager — the
    /// same-process duplicate is the only clash macOS can detect.
    /// Skipped (not failed) when the OS refuses to host a manager at
    /// all, e.g. a bare CI runner.
    #[test]
    fn probe_accepts_free_and_rejects_armed_combos() {
        let Ok(mut manager) = HotkeyManager::new() else {
            eprintln!("global hotkeys unavailable in this environment — skipping");
            return;
        };
        let raw = "control+shift+KeyX";
        assert_eq!(manager.probe(raw), Ok(()));
        manager.sync(&BTreeMap::from([(
            HotkeyAction::OpenViewer,
            raw.to_owned(),
        )]));
        assert_eq!(manager.probe(raw), Err(HotkeyError::Taken));
    }

    /// The raw accelerator produced by `HotKey::new(..).to_string()`
    /// round-trips through the parser — the persisted value must come
    /// back as the same combo when the settings re-arm at startup.
    #[test]
    fn to_string_round_trips_through_parse() {
        use global_hotkey::hotkey::{Code, Modifiers};
        for (mods, code) in [
            (Modifiers::ALT | Modifiers::SHIFT, Code::KeyE),
            (Modifiers::META | Modifiers::SHIFT, Code::KeyR),
            (Modifiers::CONTROL | Modifiers::ALT, Code::Digit1),
        ] {
            let hk = HotKey::new(Some(mods), code);
            let raw = hk.to_string();
            let parsed = raw.parse::<HotKey>().unwrap();
            assert_eq!(parsed.mods, mods);
            assert_eq!(parsed.key, code);
        }
    }

    /// The recording flow end-to-end: a combo probed in the settings
    /// (register + unregister on the live manager) must arm cleanly
    /// when the settings apply re-syncs it — a stale OS registration
    /// would make the second register fail with `Taken`.
    #[test]
    fn probed_combo_arms_cleanly_on_sync() {
        let Ok(mut manager) = HotkeyManager::new() else {
            eprintln!("global hotkeys unavailable in this environment — skipping");
            return;
        };
        let raw = "shift+meta+KeyA";
        assert_eq!(manager.probe(raw), Ok(()));
        let errors = manager.sync(&BTreeMap::from([(
            HotkeyAction::CaptureRegion,
            raw.to_owned(),
        )]));
        assert!(
            errors.is_empty(),
            "sync after probe must not report errors, got {errors:?}"
        );
    }

    /// The macOS confirmation registration is transient: while it is
    /// active the combo counts as taken (a second register would
    /// fail), and releasing it frees the combo again.
    #[test]
    fn verify_registration_is_transient() {
        let Ok(mut manager) = HotkeyManager::new() else {
            eprintln!("global hotkeys unavailable in this environment — skipping");
            return;
        };
        let raw = "shift+meta+KeyV";
        let hk = raw.parse().unwrap();
        manager.register_verify(hk, HotkeyAction::OpenViewer);
        assert_eq!(manager.probe(raw), Err(HotkeyError::Taken));
        manager.release_verify();
        assert_eq!(manager.probe(raw), Ok(()));
    }

    /// `register_verify` refuses an already-armed combo: the lib
    /// tracks one ref per id, so registering again would orphan the
    /// live Carbon registration and `release_verify` would kill it.
    /// The armed binding must survive the call untouched.
    #[test]
    fn verify_refuses_armed_combo() {
        let Ok(mut manager) = HotkeyManager::new() else {
            eprintln!("global hotkeys unavailable in this environment — skipping");
            return;
        };
        let raw = "shift+meta+KeyB";
        manager.sync(&BTreeMap::from([(
            HotkeyAction::OpenViewer,
            raw.to_owned(),
        )]));
        let hk = raw.parse().unwrap();
        assert!(manager.is_armed(&hk));
        manager.register_verify(hk, HotkeyAction::CaptureRegion);
        manager.release_verify(); // no-op — nothing was registered
        // The armed registration was left alone: still taken.
        assert_eq!(manager.probe(raw), Err(HotkeyError::Taken));
        assert!(manager.is_armed(&hk));
    }
}
