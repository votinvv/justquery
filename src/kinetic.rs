//! SHARED pedant↔justquery — править синхронно (список общих файлов — в README).
//!
//! Custom kinetic (momentum) trackpad scrolling. Windows delivers a flick's inertia as ONE big
//! wheel event ~0.5s after the finger lifts (verified by tracing), which reads as "scroll,
//! stall, jump". So: scroll the finger phase 1:1, drop that delayed lump, and run our OWN
//! velocity-based momentum from the moment the finger lifts. Mouse-wheel (integer deltas) and
//! zoom (ctrl/⌘) are passed through untouched.

use eframe::egui;

/// The momentum engine's state. Lives on `JustQueryApp` as one field; driven from two hooks:
/// [`Self::filter_input`] in `raw_input_hook` and [`Self::grace_repaint`] in the frame body.
#[derive(Default)]
pub(crate) struct KineticScroll {
    vel: egui::Vec2,                // current momentum velocity (wheel "lines"/s, both axes)
    recent: Vec<(f64, egui::Vec2)>, // recent finger deltas (time, delta) for lift-velocity
    last_touch_t: f64,              // time of the last finger (fractional) wheel event
    touch_active: bool,             // a finger gesture is in progress (events arriving)
    prev_t: f64,                    // previous frame time (for dt)
    active_until: f64,              // keep repainting until this time (smooth flick momentum)
}

impl KineticScroll {
    /// Filter the raw wheel events: pass the finger phase 1:1, drop the OS's delayed inertia
    /// lump, and inject our own synthetic momentum deltas after the finger lifts.
    pub(crate) fn filter_input(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        const GAIN: f32 = 2.5; // finger-lift velocity → momentum (tune for flick distance)
        const DECAY: f32 = 0.04; // per-second velocity retention (lower = stops sooner)
        const LIFT_GAP: f64 = 0.05; // no finger event for this long ⇒ finger lifted
        const LUMP_GAP: f64 = 0.25; // a big delta this long after the finger ⇒ OS inertia lump
        const LUMP_MIN: f32 = 10.0; // …and at least this many lines

        let now = raw_input.time.unwrap_or(self.prev_t + 0.016);
        let dt = ((now - self.prev_t).clamp(0.001, 0.1)) as f32;
        self.prev_t = now;

        let got_finger = {
            let recent = &mut self.recent;
            let vel = &mut self.vel;
            let last_touch = &mut self.last_touch_t;
            let mut finger = false;
            raw_input.events.retain(|ev| {
                if let egui::Event::MouseWheel { delta, modifiers, .. } = ev {
                    if modifiers.command || modifiers.ctrl {
                        return true; // zoom — leave alone
                    }
                    // a precision trackpad reports fractional deltas on the axis it scrolls; a
                    // mouse wheel moves in whole "notches". Treat the event as a finger gesture if
                    // EITHER axis is fractional, so a purely horizontal swipe (dy ≈ 0, dx
                    // fractional) rides our momentum too instead of the OS's delayed lump.
                    let frac = |v: f32| (v - v.round()).abs() > 0.01;
                    let fractional = frac(delta.x) || frac(delta.y);
                    if fractional {
                        if now - *last_touch > LUMP_GAP
                            && delta.x.abs().max(delta.y.abs()) > LUMP_MIN
                        {
                            return false; // drop the OS's delayed inertia lump — we do our own
                        }
                        finger = true;
                        recent.push((now, *delta));
                        *last_touch = now;
                        return true; // finger phase: apply 1:1
                    }
                    *vel = egui::Vec2::ZERO; // a wheel notch cancels momentum, then passes through
                }
                true
            });
            finger
        };
        self.recent.retain(|(t, _)| now - *t < 0.09);

        if got_finger {
            self.vel = egui::Vec2::ZERO; // finger down → track 1:1, no momentum
            self.touch_active = true;
        } else if self.touch_active && now - self.last_touch_t > LIFT_GAP {
            // finger just lifted → launch momentum from the recent finger velocity (no OS pause)
            self.touch_active = false;
            if let Some((t0, _)) = self.recent.first().copied() {
                let span = (now - t0).max(0.001) as f32;
                let sum = self.recent.iter().fold(egui::Vec2::ZERO, |a, (_, d)| a + *d);
                self.vel = (sum / span) * GAIN;
            }
            self.recent.clear();
        }

        // step the momentum: inject a synthetic wheel delta (both axes) for this frame, then decay
        if self.vel.length() > 2.0 {
            raw_input.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: self.vel * dt,
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
            self.vel *= DECAY.powf(dt);
            ctx.request_repaint(); // keep frames flowing for the whole glide
        } else {
            self.vel = egui::Vec2::ZERO;
        }
    }

    /// Keep frames flowing for a short window after ANY scroll signal. eframe repaints reactively,
    /// but a trackpad *flick* delivers inertial events in bursts with gaps — between them no
    /// repaint is requested, so the smoothing animation freezes → visible stutter (CPU is ~0.3ms,
    /// so this is purely a frame-cadence problem, not a rendering-cost one). A 0.3s grace bridges
    /// the gaps so the whole momentum phase animates smoothly.
    pub(crate) fn grace_repaint(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let scroll_signal = ctx.input(|i| {
            i.is_scrolling()
                || i.smooth_scroll_delta != egui::Vec2::ZERO
                || i.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))
        });
        if scroll_signal {
            self.active_until = now + 0.3;
        }
        if now < self.active_until {
            ctx.request_repaint();
        }
    }
}
