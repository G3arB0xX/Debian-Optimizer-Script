use std::time::{Duration, Instant};

/// Default wall-clock transition duration on local terminals (220ms).
pub const DEFAULT_TRANSITION_DURATION: Duration = Duration::from_millis(220);
/// Slightly brisker transition duration on SSH sessions (180ms).
pub const SSH_TRANSITION_DURATION: Duration = Duration::from_millis(180);
/// Active animation frame cadence (~22.2fps, 45ms) — only ticked during active transitions.
pub const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_millis(45);
/// Default floating toast notification lifetime (3.5s).
pub const TOAST_TTL_DURATION: Duration = Duration::from_millis(3500);
/// Local live telemetry polling cadence (2.0s).
pub const LOCAL_TELEMETRY_INTERVAL: Duration = Duration::from_millis(2000);
/// Remote SSH live telemetry polling cadence (3.0s).
pub const SSH_TELEMETRY_INTERVAL: Duration = Duration::from_millis(3000);
/// Draw latency threshold above which transitions auto-snap on slow SSH links (40ms).
pub const SSH_SLOW_DRAW_THRESHOLD: Duration = Duration::from_millis(40);

/// Semantic UI transition kind driven by `MotionTransition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    TabSwitch { dir: i8 },
    MenuPush,
    MenuPop,
    ModalOpen,
}

/// Wall-clock driven non-linear UI transition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionTransition {
    pub kind: TransitionKind,
    pub started_at: Instant,
    pub duration: Duration,
    pub preview_progress: Option<f64>,
}

impl MotionTransition {
    /// Starts a live wall-clock transition at `now`.
    pub fn start(kind: TransitionKind, duration: Duration, now: Instant) -> Self {
        Self {
            kind,
            started_at: now,
            duration,
            preview_progress: None,
        }
    }

    /// Creates a deterministic fixed-progress transition for `ui-preview` and tests.
    pub fn preview(kind: TransitionKind, raw_progress: f64) -> Self {
        let clamped = if raw_progress.is_nan() {
            1.0
        } else {
            raw_progress.clamp(0.0, 1.0)
        };
        Self {
            kind,
            started_at: Instant::now(),
            duration: DEFAULT_TRANSITION_DURATION,
            preview_progress: Some(clamped),
        }
    }

    /// Returns clamped linear progress in `[0.0, 1.0]` at `now`.
    pub fn raw_progress_at(&self, now: Instant) -> f64 {
        if let Some(p) = self.preview_progress {
            return if p.is_nan() { 1.0 } else { p.clamp(0.0, 1.0) };
        }
        let total = self.duration.as_secs_f64();
        if total <= 0.0 {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(self.started_at).as_secs_f64();
        (elapsed / total).clamp(0.0, 1.0)
    }

    /// Returns cubic-eased progress (`ease_out_cubic`) in `[0.0, 1.0]` at `now`.
    pub fn eased_progress_at(&self, now: Instant) -> f64 {
        ease_out_cubic(self.raw_progress_at(now))
    }

    /// Returns `true` once the transition has reached or passed its deadline.
    pub fn is_finished_at(&self, now: Instant) -> bool {
        if let Some(p) = self.preview_progress {
            return p.is_nan() || p >= 1.0;
        }
        now.saturating_duration_since(self.started_at) >= self.duration
    }

    /// Returns remaining wall-clock duration until the transition completes.
    pub fn remaining_at(&self, now: Instant) -> Duration {
        if let Some(p) = self.preview_progress {
            if p.is_nan() || p >= 1.0 {
                return Duration::ZERO;
            }
            let frac = (1.0 - p.clamp(0.0, 1.0)).clamp(0.0, 1.0);
            return Duration::from_secs_f64(self.duration.as_secs_f64() * frac);
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        self.duration.saturating_sub(elapsed)
    }
}

/// Environment-aware motion and telemetry cadence policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionPolicy {
    pub animations_enabled: bool,
    pub transition_duration: Duration,
    pub frame_interval: Duration,
    pub telemetry_interval: Option<Duration>,
    pub slow_draw_threshold: Duration,
    pub is_ssh: bool,
}

impl Default for MotionPolicy {
    fn default() -> Self {
        Self::static_disabled()
    }
}

impl MotionPolicy {
    /// Detects motion policy from the live process environment.
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let term = std::env::var("TERM").ok();
        let ci = std::env::var("CI").ok();
        let reduce_motion = std::env::var("DEBOPTY_REDUCE_MOTION").ok();
        let ssh_env = std::env::var("SSH_CONNECTION")
            .ok()
            .or_else(|| std::env::var("SSH_TTY").ok())
            .or_else(|| std::env::var("SSH_CLIENT").ok());
        Self::detect_from(
            no_color,
            term.as_deref(),
            ci.as_deref(),
            reduce_motion.as_deref(),
            ssh_env.as_deref(),
        )
    }

    /// Pure deterministic policy detection for unit testing.
    pub fn detect_from(
        no_color: bool,
        term: Option<&str>,
        ci: Option<&str>,
        reduce_motion: Option<&str>,
        ssh_env: Option<&str>,
    ) -> Self {
        let is_ssh = ssh_env.is_some_and(|v| !v.trim().is_empty());
        let is_dumb = term.is_some_and(|t| t.eq_ignore_ascii_case("dumb"));
        let is_ci = ci.is_some_and(|v| !v.trim().is_empty());
        let is_reduced = reduce_motion.is_some_and(|v| {
            let trimmed = v.trim();
            !trimmed.is_empty()
                && !trimmed.eq_ignore_ascii_case("0")
                && !trimmed.eq_ignore_ascii_case("false")
                && !trimmed.eq_ignore_ascii_case("no")
        });

        let animations_enabled = !no_color && !is_dumb && !is_ci && !is_reduced;
        let transition_duration = if is_ssh {
            SSH_TRANSITION_DURATION
        } else {
            DEFAULT_TRANSITION_DURATION
        };
        let telemetry_interval = if is_dumb || is_ci {
            None
        } else if is_ssh {
            Some(SSH_TELEMETRY_INTERVAL)
        } else {
            Some(LOCAL_TELEMETRY_INTERVAL)
        };

        Self {
            animations_enabled,
            transition_duration,
            frame_interval: ACTIVE_FRAME_INTERVAL,
            telemetry_interval,
            slow_draw_threshold: SSH_SLOW_DRAW_THRESHOLD,
            is_ssh,
        }
    }

    /// Static policy with animations and telemetry timers disabled (0 background wakes).
    pub fn static_disabled() -> Self {
        Self {
            animations_enabled: false,
            transition_duration: DEFAULT_TRANSITION_DURATION,
            frame_interval: ACTIVE_FRAME_INTERVAL,
            telemetry_interval: None,
            slow_draw_threshold: SSH_SLOW_DRAW_THRESHOLD,
            is_ssh: false,
        }
    }
}

/// Kind of scheduled renderer wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeKind {
    AnimFrame,
    TelemetrySample,
}

/// Next event-loop timer wake computed by `AppState::next_wake_at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WakeSchedule {
    pub timeout: Duration,
    pub kind: WakeKind,
}

/// Cubic ease-out curve: `1 - (1 - t)^3`, clamped to `[0.0, 1.0]` with `NaN -> 1.0`.
pub fn ease_out_cubic(t: f64) -> f64 {
    if t.is_nan() {
        return 1.0;
    }
    let clamped = t.clamp(0.0, 1.0);
    let inv = 1.0 - clamped;
    1.0 - inv * inv * inv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_checkpoints_clamping_and_nan() {
        assert!((ease_out_cubic(0.0) - 0.0).abs() < 1e-12);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-12);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-12);

        // Clamping outside [0.0, 1.0]
        assert!((ease_out_cubic(-1.5) - 0.0).abs() < 1e-12);
        assert!((ease_out_cubic(2.5) - 1.0).abs() < 1e-12);
        assert!((ease_out_cubic(f64::NEG_INFINITY) - 0.0).abs() < 1e-12);
        assert!((ease_out_cubic(f64::INFINITY) - 1.0).abs() < 1e-12);

        // NaN settles cleanly to completed (1.0)
        assert!((ease_out_cubic(f64::NAN) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn motion_transition_wall_clock_and_preview() {
        let t0 = Instant::now();
        let tr = MotionTransition::start(
            TransitionKind::TabSwitch { dir: 1 },
            Duration::from_millis(200),
            t0,
        );
        assert!(!tr.is_finished_at(t0));
        assert!((tr.raw_progress_at(t0) - 0.0).abs() < 1e-12);
        assert_eq!(tr.remaining_at(t0), Duration::from_millis(200));

        let mid = t0 + Duration::from_millis(100);
        assert!(!tr.is_finished_at(mid));
        assert!((tr.raw_progress_at(mid) - 0.5).abs() < 1e-12);
        assert!((tr.eased_progress_at(mid) - 0.875).abs() < 1e-12);
        assert_eq!(tr.remaining_at(mid), Duration::from_millis(100));

        let end = t0 + Duration::from_millis(200);
        assert!(tr.is_finished_at(end));
        assert!((tr.raw_progress_at(end) - 1.0).abs() < 1e-12);
        assert_eq!(tr.remaining_at(end), Duration::ZERO);

        let preview = MotionTransition::preview(TransitionKind::ModalOpen, 0.5);
        assert!(!preview.is_finished_at(t0));
        assert!((preview.eased_progress_at(t0) - 0.875).abs() < 1e-12);

        let preview_nan = MotionTransition::preview(TransitionKind::MenuPop, f64::NAN);
        assert!(preview_nan.is_finished_at(t0));
        assert!((preview_nan.eased_progress_at(t0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn motion_policy_detect_from_matrix() {
        let local = MotionPolicy::detect_from(false, Some("xterm-256color"), None, None, None);
        assert!(local.animations_enabled);
        assert!(!local.is_ssh);
        assert_eq!(local.transition_duration, DEFAULT_TRANSITION_DURATION);
        assert_eq!(local.telemetry_interval, Some(LOCAL_TELEMETRY_INTERVAL));

        let ssh = MotionPolicy::detect_from(
            false,
            Some("xterm-256color"),
            None,
            None,
            Some("10.0.0.1 54321 10.0.0.2 22"),
        );
        assert!(ssh.animations_enabled);
        assert!(ssh.is_ssh);
        assert_eq!(ssh.transition_duration, SSH_TRANSITION_DURATION);
        assert_eq!(ssh.telemetry_interval, Some(SSH_TELEMETRY_INTERVAL));

        let no_color = MotionPolicy::detect_from(true, Some("xterm-256color"), None, None, None);
        assert!(!no_color.animations_enabled);
        assert_eq!(no_color.telemetry_interval, Some(LOCAL_TELEMETRY_INTERVAL));

        let dumb = MotionPolicy::detect_from(false, Some("dumb"), None, None, None);
        assert!(!dumb.animations_enabled);
        assert_eq!(dumb.telemetry_interval, None);

        let ci = MotionPolicy::detect_from(false, Some("xterm-256color"), Some("true"), None, None);
        assert!(!ci.animations_enabled);
        assert_eq!(ci.telemetry_interval, None);

        let reduced =
            MotionPolicy::detect_from(false, Some("xterm-256color"), None, Some("1"), None);
        assert!(!reduced.animations_enabled);

        let reduced_false =
            MotionPolicy::detect_from(false, Some("xterm-256color"), None, Some("false"), None);
        assert!(reduced_false.animations_enabled);

        let disabled = MotionPolicy::static_disabled();
        assert!(!disabled.animations_enabled);
        assert_eq!(disabled.telemetry_interval, None);
    }
}
