//! Latency HUD data model for `mite watch`.
//!
//! Tracks per-stage pipeline timings over a rolling window and derives the two
//! things the on-overlay graph needs: a chronological time series per stage (the
//! plotted lines) and p50/p95/p99 percentiles per stage (the readout). This is
//! pure data logic with no Windows dependencies; the drawing lives in
//! `win32_overlay`, so the maths here is unit-tested in isolation.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

mod timing;

pub use timing::{StageClock, StageTimingLayer, stage_clock};

/// Hard safety cap on retained pass samples, bounding memory if passes are
/// rapid (the time window normally evicts first).
const HISTORY_CAP: usize = 10_000;

/// Percentile fractions reported per stage (p50/p95/p99).
const P50: f32 = 0.50;
const P95: f32 = 0.95;
const P99: f32 = 0.99;

/// A tracked pipeline stage. `Total` is the per-pass sum of the others and is
/// derived, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Capture,
    Detect,
    Recognize,
    Analyze,
    Present,
    Total,
}

impl Stage {
    /// The individually measured stages, in pipeline order.
    pub const TRACKED: [Stage; 5] = [
        Stage::Capture,
        Stage::Detect,
        Stage::Recognize,
        Stage::Analyze,
        Stage::Present,
    ];

    /// Every series shown on the graph / legend: the total first (drawn brightest),
    /// then each tracked stage.
    pub const ALL: [Stage; 6] = [
        Stage::Total,
        Stage::Capture,
        Stage::Detect,
        Stage::Recognize,
        Stage::Analyze,
        Stage::Present,
    ];

    /// Short label for the legend. Also the `tracing` span name used to time the
    /// stage (see [`Stage::from_span_name`]).
    pub fn label(self) -> &'static str {
        match self {
            Stage::Capture => "capture",
            Stage::Detect => "detect",
            Stage::Recognize => "recognize",
            Stage::Analyze => "analyze",
            Stage::Present => "present",
            Stage::Total => "total",
        }
    }

    /// The tracked stage a `tracing` span name refers to, if any. `Total` is
    /// derived, not a span, so it never matches.
    pub fn from_span_name(name: &str) -> Option<Stage> {
        Stage::TRACKED
            .into_iter()
            .find(|stage| stage.label() == name)
    }
}

/// One pass's per-stage wall-clock durations.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PassTimings {
    pub capture: Duration,
    pub detect: Duration,
    pub recognize: Duration,
    pub analyze: Duration,
    pub present: Duration,
}

impl PassTimings {
    /// Milliseconds for a single stage (or the summed total).
    pub fn stage_ms(&self, stage: Stage) -> f32 {
        let duration = match stage {
            Stage::Capture => self.capture,
            Stage::Detect => self.detect,
            Stage::Recognize => self.recognize,
            Stage::Analyze => self.analyze,
            Stage::Present => self.present,
            Stage::Total => return self.total_ms(),
        };
        duration.as_secs_f32() * 1000.0
    }

    /// End-to-end milliseconds: every tracked stage summed.
    pub fn total_ms(&self) -> f32 {
        Stage::TRACKED
            .iter()
            .map(|&stage| self.stage_ms(stage))
            .sum()
    }
}

/// (p50, p95, p99) milliseconds for a stage over the rolling window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Percentiles {
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
}

/// Latest-pass content counts shown alongside the latency graph.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassCounts {
    /// OCR text lines recognized this pass.
    pub lines: usize,
    /// Word highlights drawn (segmented tokens).
    pub words: usize,
    /// Of those, words found in the dictionary.
    pub known: usize,
}

/// Per-pass capture-health extras (not stage durations): how stale the consumed
/// frame was and how many capture-source frames were dropped in favour of it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PassExtras {
    /// Age of the consumed frame when it reached OCR (capture staleness).
    pub staging_age: Duration,
    /// Capture-source frames delivered since the previous pass (WGC drop count).
    pub frames_delivered: u32,
}

impl PassExtras {
    fn staging_age_ms(&self) -> f32 {
        self.staging_age.as_secs_f32() * 1000.0
    }
}

/// A time-windowed rolling history of recent pass timings: entries older than
/// `window` (relative to the newest sample) are evicted, so the graph and
/// percentiles always cover just the last `window` of activity.
#[derive(Debug)]
pub struct LatencyHud {
    history: VecDeque<(Instant, PassTimings, PassExtras)>,
    window: Duration,
    /// Hard safety cap on retained samples (bounds memory if passes are rapid).
    cap: usize,
    counts: PassCounts,
}

impl LatencyHud {
    pub fn new(window: Duration) -> Self {
        Self {
            history: VecDeque::new(),
            window,
            cap: HISTORY_CAP,
            counts: PassCounts::default(),
        }
    }

    /// Append a pass (stamped now) with its content counts and capture extras,
    /// evicting entries older than the window.
    pub fn record(&mut self, timings: PassTimings, counts: PassCounts, extras: PassExtras) {
        self.record_at(Instant::now(), timings, counts, extras);
    }

    fn record_at(
        &mut self,
        at: Instant,
        timings: PassTimings,
        counts: PassCounts,
        extras: PassExtras,
    ) {
        self.counts = counts;
        self.history.push_back((at, timings, extras));
        while let Some(&(stamp, _, _)) = self.history.front() {
            if at.saturating_duration_since(stamp) > self.window {
                self.history.pop_front();
            } else {
                break;
            }
        }
        while self.history.len() > self.cap {
            self.history.pop_front();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    pub fn len(&self) -> usize {
        self.history.len()
    }

    pub fn window(&self) -> Duration {
        self.window
    }

    /// Latest-pass content counts (lines / words / known).
    pub fn counts(&self) -> PassCounts {
        self.counts
    }

    /// A stage's values in chronological (oldest → newest) order.
    pub fn series(&self, stage: Stage) -> Vec<f32> {
        self.history
            .iter()
            .map(|(_, timings, _)| timings.stage_ms(stage))
            .collect()
    }

    /// Mean capture-source frames delivered per pass over the window. For WGC this
    /// is the average "drop count" — frames discarded in favour of the freshest.
    /// A creep toward 1 would mean we stopped keeping up; a spike in per-frame work
    /// (the bug this guards against) shows as the consumer falling behind.
    pub fn avg_frames_delivered(&self) -> f32 {
        if self.history.is_empty() {
            return 0.0;
        }
        let total: u64 = self
            .history
            .iter()
            .map(|(_, _, extras)| u64::from(extras.frames_delivered))
            .sum();
        total as f32 / self.history.len() as f32
    }

    /// p50/p95/p99 of consumed-frame staleness (ms) over the window.
    pub fn staging_age_percentiles(&self) -> Percentiles {
        let mut values: Vec<f32> = self
            .history
            .iter()
            .map(|(_, _, extras)| extras.staging_age_ms())
            .collect();
        if values.is_empty() {
            return Percentiles::default();
        }
        values.sort_by(f32::total_cmp);
        Percentiles {
            p50: nearest_rank(&values, P50),
            p95: nearest_rank(&values, P95),
            p99: nearest_rank(&values, P99),
        }
    }

    /// Effective OCR throughput: passes per second across the window's actual
    /// span. Surfaces the real consume rate (e.g. the refresh throttle), which
    /// per-stage latencies don't reveal.
    pub fn passes_per_sec(&self) -> f32 {
        if self.history.len() < 2 {
            return 0.0;
        }
        let first = self.history.front().expect("len >= 2").0;
        let last = self.history.back().expect("len >= 2").0;
        let span = last.saturating_duration_since(first).as_secs_f32();
        if span <= 0.0 {
            return 0.0;
        }
        (self.history.len() - 1) as f32 / span
    }

    /// p50/p95/p99 of a stage over the window (nearest-rank; zeros when empty).
    pub fn percentiles(&self, stage: Stage) -> Percentiles {
        let mut values: Vec<f32> = self.series(stage);
        if values.is_empty() {
            return Percentiles::default();
        }
        values.sort_by(f32::total_cmp);
        Percentiles {
            p50: nearest_rank(&values, P50),
            p95: nearest_rank(&values, P95),
            p99: nearest_rank(&values, P99),
        }
    }

    /// Largest value across the window, used to scale the graph's y-axis. The
    /// total dominates every stage, so its max is the overall max.
    pub fn max_ms(&self) -> f32 {
        self.history
            .iter()
            .map(|(_, timings, _)| timings.total_ms())
            .fold(0.0, f32::max)
    }

    /// A compact one-line p50/p95/p99 report (the same data the HUD draws),
    /// for headless logging. Milliseconds rounded to integers.
    pub fn report(&self) -> String {
        use std::fmt::Write as _;
        let mut line = format!("metrics[{}s n={}]", self.window.as_secs(), self.len());
        for stage in Stage::ALL {
            let p = self.percentiles(stage);
            let _ = write!(
                line,
                " {} {:.0}/{:.0}/{:.0}",
                stage.label(),
                p.p50,
                p.p95,
                p.p99
            );
        }
        let counts = self.counts;
        let _ = write!(
            line,
            " | lines {} words {} known {}",
            counts.lines, counts.words, counts.known
        );
        let age = self.staging_age_percentiles();
        let _ = write!(
            line,
            " | fps {:.1} drop {:.1} age {:.0}/{:.0}ms",
            self.passes_per_sec(),
            self.avg_frames_delivered(),
            age.p50,
            age.p95,
        );
        line
    }
}

/// Nearest-rank percentile of an ascending-sorted slice. `q` is a fraction in
/// `[0, 1]`.
fn nearest_rank(sorted: &[f32], q: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    let rank = (q * n as f32).ceil() as usize;
    let index = rank.clamp(1, n) - 1;
    sorted[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(capture: u64, detect: u64, recognize: u64, analyze: u64, present: u64) -> PassTimings {
        PassTimings {
            capture: Duration::from_millis(capture),
            detect: Duration::from_millis(detect),
            recognize: Duration::from_millis(recognize),
            analyze: Duration::from_millis(analyze),
            present: Duration::from_millis(present),
        }
    }

    #[test]
    fn total_sums_all_stages() {
        let t = pass(1, 2, 3, 4, 5);
        assert_eq!(t.total_ms(), 15.0);
        assert_eq!(t.stage_ms(Stage::Total), 15.0);
        assert_eq!(t.stage_ms(Stage::Detect), 2.0);
    }

    #[test]
    fn record_evicts_entries_past_window() {
        let mut hud = LatencyHud::new(Duration::from_secs(30));
        let base = Instant::now();
        // Stamps at 0/10/20/40s; evaluated relative to the newest (40s).
        for (secs, ms) in [(0, 10), (10, 20), (20, 30), (40, 40)] {
            hud.record_at(
                base + Duration::from_secs(secs),
                pass(ms, 0, 0, 0, 0),
                PassCounts::default(),
                PassExtras::default(),
            );
        }
        // Only the 0s entry is >30s older than the newest (40s) → evicted.
        assert_eq!(hud.len(), 3);
        assert_eq!(hud.series(Stage::Capture), vec![20.0, 30.0, 40.0]);
    }

    #[test]
    fn series_is_chronological() {
        let mut hud = LatencyHud::new(Duration::from_secs(30));
        hud.record(
            pass(0, 1, 0, 0, 0),
            PassCounts::default(),
            PassExtras::default(),
        );
        hud.record(
            pass(0, 2, 0, 0, 0),
            PassCounts::default(),
            PassExtras::default(),
        );
        hud.record(
            pass(0, 3, 0, 0, 0),
            PassCounts::default(),
            PassExtras::default(),
        );
        assert_eq!(hud.series(Stage::Detect), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn percentiles_nearest_rank() {
        let mut hud = LatencyHud::new(Duration::from_secs(3600));
        for ms in 1..=100 {
            hud.record(
                pass(ms as u64, 0, 0, 0, 0),
                PassCounts::default(),
                PassExtras::default(),
            );
        }
        let p = hud.percentiles(Stage::Capture);
        // Nearest-rank over 1..=100: ceil(q*100) gives the value directly.
        assert_eq!(p.p50, 50.0);
        assert_eq!(p.p95, 95.0);
        assert_eq!(p.p99, 99.0);
    }

    #[test]
    fn percentiles_handle_unsorted_input() {
        let mut hud = LatencyHud::new(Duration::from_secs(30));
        for ms in [30, 10, 50, 20, 40] {
            hud.record(
                pass(ms, 0, 0, 0, 0),
                PassCounts::default(),
                PassExtras::default(),
            );
        }
        // Sorted: 10,20,30,40,50. p50 -> ceil(.5*5)=3rd -> 30.
        assert_eq!(hud.percentiles(Stage::Capture).p50, 30.0);
    }

    #[test]
    fn record_tracks_latest_counts() {
        let mut hud = LatencyHud::new(Duration::from_secs(30));
        hud.record(
            pass(1, 0, 0, 0, 0),
            PassCounts {
                lines: 3,
                words: 17,
                known: 12,
            },
            PassExtras::default(),
        );
        assert_eq!(
            hud.counts(),
            PassCounts {
                lines: 3,
                words: 17,
                known: 12
            }
        );
    }

    #[test]
    fn empty_hud_is_safe() {
        let hud = LatencyHud::new(Duration::from_secs(30));
        assert!(hud.is_empty());
        assert_eq!(hud.max_ms(), 0.0);
        assert_eq!(hud.percentiles(Stage::Total), Percentiles::default());
        assert!(hud.series(Stage::Total).is_empty());
        assert_eq!(hud.counts(), PassCounts::default());
    }

    #[test]
    fn max_ms_tracks_total() {
        let mut hud = LatencyHud::new(Duration::from_secs(30));
        hud.record(
            pass(1, 1, 1, 1, 1),
            PassCounts::default(),
            PassExtras::default(),
        ); // total 5
        hud.record(
            pass(2, 2, 2, 2, 2),
            PassCounts::default(),
            PassExtras::default(),
        ); // total 10
        assert_eq!(hud.max_ms(), 10.0);
    }

    fn extras(age_ms: u64, frames_delivered: u32) -> PassExtras {
        PassExtras {
            staging_age: Duration::from_millis(age_ms),
            frames_delivered,
        }
    }

    #[test]
    fn aggregates_capture_extras() {
        let mut hud = LatencyHud::new(Duration::from_secs(30));
        let base = Instant::now();
        hud.record_at(
            base,
            pass(0, 0, 0, 0, 0),
            PassCounts::default(),
            extras(10, 30),
        );
        hud.record_at(
            base + Duration::from_secs(1),
            pass(0, 0, 0, 0, 0),
            PassCounts::default(),
            extras(20, 40),
        );
        assert_eq!(hud.avg_frames_delivered(), 35.0);
        // Two samples 1s apart → (2-1)/1s = 1 pass/sec.
        assert_eq!(hud.passes_per_sec(), 1.0);
        // Nearest-rank over [10, 20]: p50 → ceil(.5*2)=1st → 10, p95 → 20.
        let age = hud.staging_age_percentiles();
        assert_eq!(age.p50, 10.0);
        assert_eq!(age.p95, 20.0);
    }

    #[test]
    fn extras_safe_when_empty() {
        let hud = LatencyHud::new(Duration::from_secs(30));
        assert_eq!(hud.avg_frames_delivered(), 0.0);
        assert_eq!(hud.passes_per_sec(), 0.0);
        assert_eq!(hud.staging_age_percentiles(), Percentiles::default());
    }
}
