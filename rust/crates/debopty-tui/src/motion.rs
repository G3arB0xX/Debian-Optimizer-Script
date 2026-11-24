use std::time::{Duration, Instant};

/// Default wall-clock transition duration on local terminals (220ms).
pub const DEFAULT_TRANSITION_DURATION: Duration = Duration::from_millis(220);
/// Slightly brisker transition duration on SSH sessions (180ms).
pub const SSH_TRANSITION_DURATION: Duration = Duration::from_millis(180);
/// Reduced-motion transition duration when `DEBOPTY_MOTION=reduced` (120ms).
pub const REDUCED_TRANSITION_DURATION: Duration = Duration::from_millis(120);
/// Legacy compatibility active animation frame cadence (~22.2fps, 45ms).
pub const ACTIVE_FRAME_INTERVAL: Duration = Duration::from_millis(45);
/// Smooth local interactive animation frame cadence (~60fps, 16ms).
pub const LOCAL_FRAME_INTERVAL: Duration = Duration::from_millis(16);
/// Remote SSH animation frame cadence (~30fps, 33ms) balancing fluidity and bandwidth.
pub const SSH_FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// Local live task log stream redraw cadence (~30fps, 33ms).
pub const LOCAL_STREAM_INTERVAL: Duration = Duration::from_millis(33);
/// Remote SSH live task log stream redraw cadence (~18fps, 55ms) preventing log-burst stalls.
pub const SSH_STREAM_INTERVAL: Duration = Duration::from_millis(55);
/// Default floating toast notification lifetime (3.5s).
pub const TOAST_TTL_DURATION: Duration = Duration::from_millis(3500);
/// Floating toast eased reveal duration (140ms).
pub const TOAST_REVEAL_DURATION: Duration = Duration::from_millis(140);
/// Floating toast eased fade-out duration before expiry (200ms).
pub const TOAST_FADE_DURATION: Duration = Duration::from_millis(200);
/// Local live telemetry polling cadence (2.0s).
pub const LOCAL_TELEMETRY_INTERVAL: Duration = Duration::from_millis(2000);
/// Remote SSH live telemetry polling cadence (3.0s).
pub const SSH_TELEMETRY_INTERVAL: Duration = Duration::from_millis(3000);
/// Draw latency threshold above which transitions auto-snap on sustained slow links (40ms).
pub const SSH_SLOW_DRAW_THRESHOLD: Duration = Duration::from_millis(40);

/// Explicit user-selectable motion preference parsed from `DEBOPTY_MOTION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionMode {
    /// Automatic environment-aware cadence (60fps local, 30fps SSH).
    #[default]
    Auto,
    /// Prefer high-fluidity 60fps cadence even over SSH (`DEBOPTY_MOTION=smooth`).
    Smooth,
    /// Brisk, lower-cadence transitions (`DEBOPTY_MOTION=reduced`).
    Reduced,
    /// Disable animated transitions completely (`DEBOPTY_MOTION=off`).
    Off,
}

impl MotionMode {
    /// Parses `DEBOPTY_MOTION` env value (`auto|smooth|reduced|off`), defaulting to `Auto`.
    pub fn parse(raw: Option<&str>) -> Self {
        let Some(val) = raw.map(str::trim) else {
            return Self::Auto;
        };
        match val.to_ascii_lowercase().as_str() {
            "smooth" | "high" | "60" | "on" => Self::Smooth,
            "reduced" | "low" | "minimal" => Self::Reduced,
            "off" | "none" | "0" | "false" | "disabled" => Self::Off,
            _ => Self::Auto,
        }
    }
}

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
        let motion_mode = std::env::var("DEBOPTY_MOTION").ok();
        Self::detect_from_with_mode(
            no_color,
            term.as_deref(),
            ci.as_deref(),
            reduce_motion.as_deref(),
            ssh_env.as_deref(),
            motion_mode.as_deref(),
        )
    }

    /// Pure deterministic policy detection for unit testing (5-arg backward-compatible signature).
    pub fn detect_from(
        no_color: bool,
        term: Option<&str>,
        ci: Option<&str>,
        reduce_motion: Option<&str>,
        ssh_env: Option<&str>,
    ) -> Self {
        Self::detect_from_with_mode(no_color, term, ci, reduce_motion, ssh_env, None)
    }

    /// Pure deterministic policy detection honoring `DEBOPTY_MOTION=auto|smooth|reduced|off`.
    pub fn detect_from_with_mode(
        no_color: bool,
        term: Option<&str>,
        ci: Option<&str>,
        reduce_motion: Option<&str>,
        ssh_env: Option<&str>,
        motion_mode: Option<&str>,
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
        let mode = MotionMode::parse(motion_mode);

        let base_allowed = !no_color && !is_dumb && !is_ci && !is_reduced;
        let animations_enabled = base_allowed && mode != MotionMode::Off;
        let transition_duration = match mode {
            MotionMode::Smooth => DEFAULT_TRANSITION_DURATION,
            MotionMode::Reduced => REDUCED_TRANSITION_DURATION,
            MotionMode::Auto | MotionMode::Off => {
                if is_ssh {
                    SSH_TRANSITION_DURATION
                } else {
                    DEFAULT_TRANSITION_DURATION
                }
            }
        };
        let frame_interval = match mode {
            MotionMode::Smooth => LOCAL_FRAME_INTERVAL,
            MotionMode::Reduced => ACTIVE_FRAME_INTERVAL,
            MotionMode::Auto | MotionMode::Off => {
                if is_ssh {
                    SSH_FRAME_INTERVAL
                } else {
                    LOCAL_FRAME_INTERVAL
                }
            }
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
            frame_interval,
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

/// Quartic ease-out curve: `1 - (1 - t)^4`, clamped to `[0.0, 1.0]` with `NaN -> 1.0`.
/// Delivers a snappy initial launch with a silky long settle for page and modal transitions.
pub fn ease_out_quart(t: f64) -> f64 {
    if t.is_nan() {
        return 1.0;
    }
    let clamped = t.clamp(0.0, 1.0);
    let inv = 1.0 - clamped;
    let inv2 = inv * inv;
    1.0 - inv2 * inv2
}

/// Adaptive frame-cadence pacer tracking rolling EMA draw latency over SSH/local links.
///
/// Instead of abruptly aborting animations on a single transient draw spike, `FramePacer`
/// smoothly widens the frame interval (`base -> 33ms -> 50ms -> 66ms`) under sustained pressure
/// and signals a transition snap only on sustained slowness (`>= 2` consecutive slow draws,
/// EMA >= `slow_threshold`, or a single extreme stall `>= 2 * slow_threshold`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePacer {
    base_interval: Duration,
    slow_threshold: Duration,
    ema_draw_us: u64,
    consecutive_slow: u8,
    samples: u64,
}

impl FramePacer {
    pub fn new(policy: MotionPolicy) -> Self {
        Self {
            base_interval: policy.frame_interval,
            slow_threshold: policy.slow_draw_threshold,
            ema_draw_us: 0,
            consecutive_slow: 0,
            samples: 0,
        }
    }

    /// Updates target base interval and slow threshold while preserving rolling EMA history.
    pub fn update_policy(&mut self, policy: MotionPolicy) {
        self.base_interval = policy.frame_interval;
        self.slow_threshold = policy.slow_draw_threshold;
    }

    /// Records one completed frame draw duration and returns `true` when sustained slowness
    /// warrants snapping any active transition to its settled end state.
    pub fn record_draw(&mut self, elapsed: Duration) -> bool {
        let us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        if self.samples == 0 {
            self.ema_draw_us = us;
        } else {
            // 25% weight for newest sample (alpha = 1/4) balances responsiveness and stability.
            self.ema_draw_us = (self.ema_draw_us.saturating_mul(3).saturating_add(us)) / 4;
        }
        self.samples = self.samples.saturating_add(1);

        if elapsed >= self.slow_threshold {
            self.consecutive_slow = self.consecutive_slow.saturating_add(1);
        } else {
            self.consecutive_slow = 0;
        }

        let ema = Duration::from_micros(self.ema_draw_us);
        let extreme_spike = self.slow_threshold.saturating_mul(2);
        self.consecutive_slow >= 2 || ema >= self.slow_threshold || elapsed >= extreme_spike
    }

    /// Returns the smoothed animation frame interval, stepping down under load and relaxing
    /// back to `base_interval` once draws become fast again.
    pub fn effective_frame_interval(&self) -> Duration {
        let ema = Duration::from_micros(self.ema_draw_us);
        let three_quarter = self.slow_threshold.saturating_mul(3) / 4;
        let half = self.slow_threshold / 2;

        if ema >= self.slow_threshold {
            self.base_interval.max(Duration::from_millis(66))
        } else if ema >= three_quarter || self.consecutive_slow >= 2 {
            self.base_interval.max(Duration::from_millis(50))
        } else if ema >= half || self.consecutive_slow >= 1 {
            self.base_interval.max(Duration::from_millis(33))
        } else {
            self.base_interval
        }
    }

    /// Returns the recommended minimum interval between live worker-log redraws.
    pub fn recommended_stream_interval(&self, is_ssh: bool) -> Duration {
        let base = if is_ssh {
            SSH_STREAM_INTERVAL
        } else {
            LOCAL_STREAM_INTERVAL
        };
        let ema = Duration::from_micros(self.ema_draw_us);
        let three_quarter = self.slow_threshold.saturating_mul(3) / 4;
        let half = self.slow_threshold / 2;

        if ema >= self.slow_threshold {
            base.max(Duration::from_millis(120))
        } else if ema >= three_quarter {
            base.max(Duration::from_millis(90))
        } else if ema >= half {
            base.max(Duration::from_millis(66))
        } else {
            base.max(self.effective_frame_interval())
        }
    }

    /// Returns the current rolling EMA draw duration.
    pub fn ema_draw_duration(&self) -> Duration {
        Duration::from_micros(self.ema_draw_us)
    }

    /// Returns the current consecutive slow-draw streak count.
    pub fn consecutive_slow(&self) -> u8 {
        self.consecutive_slow
    }
}

/// Pure detector for DEC Mode 2026 Synchronized Output (`\x1b[?2026h`..`\x1b[?2026l`) support.
///
/// Returns `false` for legacy/dumb terminals (`TERM=dumb`, `TERM=linux`, `TERM=vt100`) or when
/// `DEBOPTY_NO_SYNC_OUTPUT` is set to a truthy value; returns `true` for modern terminals
/// (`xterm*`, `screen*`, `tmux*`, `alacritty*`, `wezterm*`, `kitty*`, `ghostty*`, `foot*`,
/// `rxvt*`, etc.) or unset interactive TTYs.
pub fn supports_synchronized_output(term: Option<&str>, no_sync_env: Option<&str>) -> bool {
    let opted_out = no_sync_env.is_some_and(|v| {
        let trimmed = v.trim();
        !trimmed.is_empty()
            && !trimmed.eq_ignore_ascii_case("0")
            && !trimmed.eq_ignore_ascii_case("false")
            && !trimmed.eq_ignore_ascii_case("no")
            && !trimmed.eq_ignore_ascii_case("off")
    });
    if opted_out {
        return false;
    }

    let Some(raw_term) = term.map(str::trim) else {
        return true;
    };
    if raw_term.is_empty() {
        return true;
    }
    let lower = raw_term.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "dumb" | "linux" | "vt100" | "vt102" | "vt220" | "cons25"
    ) {
        return false;
    }
    lower.starts_with("xterm")
        || lower.starts_with("screen")
        || lower.starts_with("tmux")
        || lower.starts_with("alacritty")
        || lower.starts_with("wezterm")
        || lower.starts_with("kitty")
        || lower.starts_with("ghostty")
        || lower.starts_with("foot")
        || lower.starts_with("rxvt")
        || lower.starts_with("rio")
        || lower.starts_with("contour")
        || lower.starts_with("mintty")
        || lower.starts_with("iterm")
        || lower.starts_with("vte")
        || lower.starts_with("konsole")
        || lower.starts_with("st-")
        || lower == "st"
        || lower.contains("256color")
        || lower.contains("direct")
}

/// Detects DEC Mode 2026 Synchronized Output support from the live environment.
pub fn detect_synchronized_output() -> bool {
    let term = std::env::var("TERM").ok();
    let no_sync = std::env::var("DEBOPTY_NO_SYNC_OUTPUT").ok();
    supports_synchronized_output(term.as_deref(), no_sync.as_deref())
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
