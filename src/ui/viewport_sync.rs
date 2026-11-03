//! Viewport geometry commands sent sparingly — and audited.
//!
//! On Windows a `WM_DPICHANGED` makes egui's reported
//! `inner_rect`/`outer_rect` transiently inconsistent with the physical
//! window: comparing a desired size against the *reported* size then
//! re-sends `InnerSize`/`OuterPosition` mid-transition, which fights the
//! OS move/resize loop (mixed-DPI drag: the window grows or the drag
//! grip detaches). The fix, per the DPI audit: compare against the last
//! size/position WE requested, with a one-point tolerance, and log any
//! burst of commands — a feedback loop shows up as a storm here.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use egui::{Context, Pos2, Vec2, ViewportCommand};

/// Don't resend a command whose target moved less than this (egui pt).
const TOLERANCE: f32 = 1.0;
/// More than this many geometry commands inside `BURST_WINDOW` means a
/// resize/reposition feedback loop — logged once per burst.
const BURST_COUNT: usize = 6;
const BURST_WINDOW: Duration = Duration::from_millis(800);

/// Whether `target` differs from the last requested value by more than
/// the one-point tolerance (`None` = nothing requested yet → send).
fn size_changed(last: Option<Vec2>, target: Vec2) -> bool {
    last.is_none_or(|l| (l - target).abs().max_elem() > TOLERANCE)
}

/// `size_changed` for positions.
fn pos_changed(last: Option<Pos2>, target: Pos2) -> bool {
    last.is_none_or(|l| (l - target).abs().max_elem() > TOLERANCE)
}

/// Rolling command-rate tracker shared by every floating window.
#[derive(Debug)]
struct Burst {
    sent: VecDeque<Instant>,
    warned: bool,
}

impl Burst {
    fn new() -> Self {
        Self {
            sent: VecDeque::with_capacity(BURST_COUNT + 1),
            warned: false,
        }
    }

    /// Records a command at `now`; returns `true` once when the rate
    /// crosses the burst threshold (re-arms after a quiet window).
    fn note(&mut self, now: Instant) -> bool {
        self.sent.push_back(now);
        while self
            .sent
            .front()
            .is_some_and(|t| now.duration_since(*t) > BURST_WINDOW)
        {
            self.sent.pop_front();
        }
        if self.sent.len() < BURST_COUNT {
            self.warned = false;
            return false;
        }
        !std::mem::replace(&mut self.warned, true)
    }
}

/// Per-window command ledger: remembers the last requested inner size and
/// outer position, dedups everything else, and warns on command bursts.
/// A fresh `ViewportSync` always sends on first use (window creation may
/// leave the reported rect unset).
#[derive(Debug)]
pub struct ViewportSync {
    name: &'static str,
    last_size: Option<Vec2>,
    last_pos: Option<Pos2>,
    burst: Burst,
}

impl ViewportSync {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            last_size: None,
            last_pos: None,
            burst: Burst::new(),
        }
    }

    /// Sends `InnerSize(target)` only when it differs from the last size
    /// THIS window asked for — never because the reported size changed
    /// (DPI transitions report transient sizes; winit applies our last
    /// request at the new scale on its own).
    pub fn request_inner_size(&mut self, ctx: &Context, target: Vec2) {
        if !size_changed(self.last_size, target) {
            return;
        }
        self.last_size = Some(target);
        self.note("InnerSize");
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
    }

    /// `request_inner_size` for `OuterPosition`.
    pub fn request_outer_position(&mut self, ctx: &Context, target: Pos2) {
        if !pos_changed(self.last_pos, target) {
            return;
        }
        self.last_pos = Some(target);
        self.note("OuterPosition");
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(target));
    }

    fn note(&mut self, what: &'static str) {
        if self.burst.note(Instant::now()) {
            tracing::warn!(
                window = self.name,
                command = what,
                "viewport command burst — possible resize/position feedback loop"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_point_tolerance_dedups_but_real_changes_send() {
        let size = Vec2::new(340.0, 76.0);
        assert!(size_changed(None, size));
        assert!(!size_changed(Some(size), size + Vec2::splat(0.9)));
        assert!(size_changed(Some(size), size + Vec2::splat(1.1)));
        let pos = Pos2::new(10.0, 20.0);
        assert!(pos_changed(None, pos));
        assert!(!pos_changed(Some(pos), pos + Vec2::splat(0.5)));
        assert!(pos_changed(Some(pos), pos + Vec2::new(2.0, 0.0)));
    }

    /// A DPI transition changes the *reported* size, not the requested
    /// one: nothing is resent while the target is unchanged.
    #[test]
    fn dpi_report_shifts_do_not_retrigger() {
        let requested = Vec2::new(340.0, 76.0);
        // egui reports 510×114 pt mid-transition (physical/1.0 before the
        // DPI resize lands): the last request still matches — no command.
        let reported_mid_transition = Vec2::new(510.0, 114.0);
        let _ = reported_mid_transition; // reported sizes are never consulted
        assert!(!size_changed(Some(requested), requested));
    }

    #[test]
    fn bursts_are_logged_once_then_rearmed() {
        let mut b = Burst::new();
        let t0 = Instant::now();
        let mut fires = 0;
        for i in 0..BURST_COUNT + 4 {
            fires += usize::from(b.note(t0 + Duration::from_millis(i as u64 * 50)));
        }
        assert_eq!(fires, 1, "one warning per burst, not per command");
        // After a quiet spell (past the last note + the window) the
        // counter drains and a new burst warns again.
        let quiet = t0 + Duration::from_millis(1600);
        for i in 0..BURST_COUNT {
            fires += usize::from(b.note(quiet + Duration::from_millis(i as u64 * 50)));
        }
        assert_eq!(fires, 2);
    }
}
