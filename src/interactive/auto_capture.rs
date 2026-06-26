//! Automatic eval-fixture capture for `watch`.
//!
//! While watching, every fresh full OCR pass yields a detection: the recognized
//! line texts plus their box rects. When that detection differs enough from the
//! scenes we have already saved (new dialogue text, or a menu/scene layout
//! shift), we save the raw frame as an eval fixture, the same artifact
//! Ctrl+Alt+S writes. Smoothing already suppresses fresh passes on a stable
//! scene, so a fresh pass that differs is a genuine change.
//!
//! Two design points keep the captures useful:
//!
//! - We wait for the scene to *settle* before saving. A detected change only
//!   arms a pending capture; the save fires once the scene is confirmed stable
//!   (the next pass reuses the detection, or two consecutive fresh passes agree
//!   when smoothing is off). This avoids saving a mid-fade, partially
//!   recognized frame. The exact frame that produced the armed detection is
//!   held (cheaply, the pixels are an `Arc`) and written, so there is no
//!   re-capture skew.
//! - We dedup against everything already saved, including previous sessions
//!   (fingerprints are loaded from `capture.json` on startup). The same
//!   similarity score that decides "this is a new scene" is replayed against
//!   the known scenes, so revisiting a menu does not re-save it.
//!
//! The scoring is pure and unit-tested here; only `try_flush` touches the
//! filesystem.

use std::collections::HashSet;
use std::hash::Hash;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::capture::Frame;
use crate::eval_capture::{self, DetectionFingerprint};
use crate::ocr::RecognizedText;

/// Box rects are bucketed at this many pixels before comparison, so subpixel
/// jitter and small layout drift do not read as a new scene.
const LAYOUT_BUCKET_PX: f32 = 16.0;

/// Tunables for the automatic-capture decision, derived from `[eval_capture]`
/// config.
#[derive(Debug, Clone, Copy)]
pub struct AutoCaptureThresholds {
    /// Save when recognized-text dissimilarity (0 = identical, 1 = disjoint)
    /// reaches this.
    pub text_change: f32,
    /// Save when box-layout dissimilarity (0 = identical, 1 = disjoint) reaches
    /// this.
    pub layout_change: f32,
    /// Minimum delay between two automatic saves.
    pub min_interval: Duration,
    /// Hard cap on automatic saves per `watch` session (0 = unlimited).
    pub max_per_session: usize,
}

/// How much one detection differs from another, split into the two channels the
/// thresholds gate on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChangeScore {
    /// `1 - Jaccard` over the recognized line sets.
    pub text: f32,
    /// `1 - Jaccard` over the quantized box-rect sets.
    pub layout: f32,
}

/// Build a [`DetectionFingerprint`] from a finished OCR pass's recognized items.
pub fn fingerprint_from_items(items: &[RecognizedText]) -> DetectionFingerprint {
    let mut lines: Vec<String> = items
        .iter()
        .map(|item| normalize_line(&item.text))
        .filter(|line| !line.is_empty())
        .collect();
    lines.sort();
    lines.dedup();

    let mut boxes: Vec<[i32; 4]> = items
        .iter()
        .map(|item| {
            let (x, y, w, h) = item.text_box.rect.quantized_key(LAYOUT_BUCKET_PX);
            [x, y, w, h]
        })
        .collect();
    boxes.sort();
    boxes.dedup();

    DetectionFingerprint { lines, boxes }
}

/// Score how much two fingerprints differ, per channel.
pub fn change_score(a: &DetectionFingerprint, b: &DetectionFingerprint) -> ChangeScore {
    ChangeScore {
        text: 1.0
            - jaccard(
                a.lines.iter().map(String::as_str),
                b.lines.iter().map(String::as_str),
            ),
        layout: 1.0 - jaccard(a.boxes.iter().copied(), b.boxes.iter().copied()),
    }
}

/// Trim, then collapse internal whitespace runs to a single space, so OCR
/// spacing noise does not change the line identity.
fn normalize_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.trim().chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

/// Jaccard similarity of two iterables treated as sets. Two empty sets are
/// defined as identical (similarity 1.0).
fn jaccard<T: Eq + Hash>(a: impl IntoIterator<Item = T>, b: impl IntoIterator<Item = T>) -> f32 {
    let a: HashSet<T> = a.into_iter().collect();
    let b: HashSet<T> = b.into_iter().collect();
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let union = a.union(&b).count();
    if union == 0 {
        return 1.0;
    }
    a.intersection(&b).count() as f32 / union as f32
}

/// An armed-but-not-yet-saved capture: the detection that tripped the change
/// threshold and the exact frame that produced it.
struct Pending {
    fingerprint: DetectionFingerprint,
    frame: Frame,
    window_id: u32,
}

/// Live automatic-capture state, owned by the watch worker and driven once per
/// OCR pass.
pub struct AutoCaptureState {
    output_dir: PathBuf,
    thresholds: AutoCaptureThresholds,
    /// Every scene saved so far, including those loaded from previous sessions.
    seen: Vec<DetectionFingerprint>,
    saved_this_session: usize,
    last_save: Option<Instant>,
    pending: Option<Pending>,
}

impl AutoCaptureState {
    /// Build the state for `output_dir`, seeding `seen` with the fingerprints of
    /// captures already on disk so we dedup across sessions.
    pub fn new(output_dir: PathBuf, thresholds: AutoCaptureThresholds) -> Self {
        let seen = eval_capture::load_existing_fingerprints(&output_dir);
        Self {
            output_dir,
            thresholds,
            seen,
            saved_this_session: 0,
            last_save: None,
            pending: None,
        }
    }

    /// Observe a fresh full detection and the frame that produced it. Returns
    /// `Some` only when this call actually wrote a capture (e.g. the previous
    /// scene had already settled).
    pub fn observe_fresh(
        &mut self,
        fingerprint: DetectionFingerprint,
        frame: &Frame,
        window_id: u32,
    ) -> Option<Result<PathBuf, String>> {
        // A blank / no-text frame is neither a useful fixture nor a scene
        // boundary; drop any pending arm so a transient blank does not flush.
        if fingerprint.lines.is_empty() {
            self.pending = None;
            return None;
        }
        // Already-known scene (this session or a previous one): nothing to do.
        if !self.is_novel(&fingerprint) {
            self.pending = None;
            return None;
        }
        if self.cap_reached() {
            return None;
        }

        // If the previous fresh pass already armed this same novel scene, two
        // consecutive reads agree and it has settled even though smoothing never
        // reused (e.g. --no-smoothing). Flush now.
        let settled_via_repeat =
            matches!(&self.pending, Some(p) if self.is_similar(&p.fingerprint, &fingerprint));
        if settled_via_repeat {
            return self.try_flush();
        }

        // First sighting of this novel scene, or the scene changed again before
        // settling: (re)arm with the latest frame and wait.
        self.pending = Some(Pending {
            fingerprint,
            frame: frame.clone(),
            window_id,
        });
        None
    }

    /// Observe a reuse/unchanged pass. The scene is confirmed stable and equals
    /// the last fresh detection, so any armed capture can flush.
    pub fn observe_stable(&mut self) -> Option<Result<PathBuf, String>> {
        if self.pending.is_some() {
            self.try_flush()
        } else {
            None
        }
    }

    /// Write the pending capture if the cooldown and per-session cap allow.
    /// Leaves the arm in place when only the cooldown blocks it, so a later
    /// stable pass can still flush it.
    fn try_flush(&mut self) -> Option<Result<PathBuf, String>> {
        if self.cap_reached() {
            self.pending = None;
            return None;
        }
        if let Some(last) = self.last_save
            && last.elapsed() < self.thresholds.min_interval
        {
            return None;
        }
        let pending = self.pending.take()?;
        match eval_capture::write_raw_capture(
            &self.output_dir,
            pending.window_id,
            &pending.frame,
            Some(&pending.fingerprint),
        ) {
            Ok(dir) => {
                self.seen.push(pending.fingerprint);
                self.saved_this_session += 1;
                self.last_save = Some(Instant::now());
                Some(Ok(dir))
            }
            Err(error) => Some(Err(format!("{error:#}"))),
        }
    }

    fn cap_reached(&self) -> bool {
        self.thresholds.max_per_session != 0
            && self.saved_this_session >= self.thresholds.max_per_session
    }

    /// Whether `a` and `b` are close enough to be the same scene (below both
    /// change thresholds).
    fn is_similar(&self, a: &DetectionFingerprint, b: &DetectionFingerprint) -> bool {
        let score = change_score(a, b);
        score.text < self.thresholds.text_change && score.layout < self.thresholds.layout_change
    }

    /// Whether `fingerprint` differs enough from every saved scene to be worth
    /// capturing.
    fn is_novel(&self, fingerprint: &DetectionFingerprint) -> bool {
        self.seen
            .iter()
            .all(|seen| !self.is_similar(seen, fingerprint))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use image::{Rgb, RgbImage};

    use super::*;
    use crate::capture::{FrameSourceKind, FrameSourceMetadata};
    use crate::geometry::{Rect, ScreenRect, Size};
    use crate::ocr::{RecognizedText, TextBox};

    fn item(text: &str, rect: Rect) -> RecognizedText {
        RecognizedText {
            text_box: TextBox {
                id: 0,
                rect,
                confidence: 1.0,
                content_fingerprint: 0,
            },
            text: text.to_string(),
            confidence: 1.0,
            reused: false,
            char_centers: Vec::new(),
        }
    }

    fn fp(lines: &[&str], boxes: &[[i32; 4]]) -> DetectionFingerprint {
        DetectionFingerprint {
            lines: lines.iter().map(|s| s.to_string()).collect(),
            boxes: boxes.to_vec(),
        }
    }

    fn frame() -> Frame {
        Frame {
            id: 1,
            captured_at: Instant::now(),
            size: Size::new(2, 1),
            screen_rect: ScreenRect::new(0, 0, Size::new(2, 1)),
            source: FrameSourceMetadata {
                kind: FrameSourceKind::WindowsGraphicsCapture,
                label: None,
                app_name: None,
                window_id: None,
                pid: None,
            },
            content_epoch: 0,
            pixels: Some(Arc::new(RgbImage::from_pixel(2, 1, Rgb([1, 2, 3])))),
            frames_delivered: 1,
            staging_age: Duration::ZERO,
        }
    }

    fn thresholds() -> AutoCaptureThresholds {
        AutoCaptureThresholds {
            text_change: 0.5,
            layout_change: 0.5,
            min_interval: Duration::ZERO,
            max_per_session: 0,
        }
    }

    fn state(dir: &std::path::Path) -> AutoCaptureState {
        AutoCaptureState::new(dir.to_path_buf(), thresholds())
    }

    fn count_captures(root: &std::path::Path) -> usize {
        std::fs::read_dir(root)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn fingerprint_normalizes_sorts_and_dedups() {
        let items = vec![
            item("  ねこ  だ ", Rect::new(0.0, 0.0, 30.0, 10.0)),
            item("あい", Rect::new(100.0, 0.0, 30.0, 10.0)),
            item("ねこ だ", Rect::new(0.0, 0.0, 30.0, 10.0)),
            item("   ", Rect::new(50.0, 0.0, 30.0, 10.0)),
        ];
        let fingerprint = fingerprint_from_items(&items);
        // Whitespace collapsed, blank dropped, duplicate "ねこ だ" merged, sorted.
        assert_eq!(
            fingerprint.lines,
            vec!["あい".to_string(), "ねこ だ".to_string()]
        );
        // Two identical rects collapse to one bucket; three distinct x positions
        // remain (including the blank line's box, which is still a layout box).
        assert_eq!(fingerprint.boxes.len(), 3);
    }

    #[test]
    fn change_score_identical_and_disjoint() {
        let a = fp(&["a", "b"], &[[0, 0, 1, 1]]);
        let identical = change_score(&a, &a);
        assert_eq!(identical.text, 0.0);
        assert_eq!(identical.layout, 0.0);

        let b = fp(&["c", "d"], &[[9, 9, 1, 1]]);
        let disjoint = change_score(&a, &b);
        assert_eq!(disjoint.text, 1.0);
        assert_eq!(disjoint.layout, 1.0);
    }

    #[test]
    fn change_score_partial_overlap() {
        let a = fp(&["a", "b"], &[]);
        let b = fp(&["b", "c"], &[]);
        // Intersection {b}=1, union {a,b,c}=3 -> similarity 1/3.
        assert!((change_score(&a, &b).text - (1.0 - 1.0 / 3.0)).abs() < 1e-6);
        // Two empty box sets are identical.
        assert_eq!(change_score(&a, &b).layout, 0.0);
    }

    #[test]
    fn arms_on_change_then_flushes_when_settled() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(dir.path());
        let scene = fp(&["新しい台詞"], &[[0, 0, 10, 1]]);

        // Fresh change only arms; nothing written yet.
        assert!(state.observe_fresh(scene.clone(), &frame(), 1).is_none());
        assert_eq!(count_captures(dir.path()), 0);

        // The scene settles -> the armed capture flushes.
        let saved = state.observe_stable().expect("a capture should flush");
        assert!(saved.is_ok());
        assert_eq!(count_captures(dir.path()), 1);
    }

    #[test]
    fn flushes_on_two_consecutive_agreeing_reads() {
        // Mirrors --no-smoothing: a reuse pass never arrives, so settling is
        // detected by two fresh reads of the same scene.
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(dir.path());
        let scene = fp(&["台詞"], &[[0, 0, 10, 1]]);

        assert!(state.observe_fresh(scene.clone(), &frame(), 1).is_none());
        let saved = state
            .observe_fresh(scene, &frame(), 1)
            .expect("second read flushes");
        assert!(saved.is_ok());
        assert_eq!(count_captures(dir.path()), 1);
    }

    #[test]
    fn blank_scene_never_captures() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(dir.path());
        assert!(state.observe_fresh(fp(&[], &[]), &frame(), 1).is_none());
        assert!(state.observe_stable().is_none());
        assert_eq!(count_captures(dir.path()), 0);
    }

    #[test]
    fn known_scene_is_not_recaptured() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = state(dir.path());
        let scene = fp(&["同じ画面"], &[[0, 0, 10, 1]]);

        state.observe_fresh(scene.clone(), &frame(), 1);
        state.observe_stable().unwrap().unwrap();
        assert_eq!(count_captures(dir.path()), 1);

        // Seeing the same scene again does not arm or save.
        assert!(state.observe_fresh(scene, &frame(), 1).is_none());
        assert!(state.observe_stable().is_none());
        assert_eq!(count_captures(dir.path()), 1);
    }

    #[test]
    fn cooldown_holds_the_capture_until_a_later_pass() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AutoCaptureState::new(
            dir.path().to_path_buf(),
            AutoCaptureThresholds {
                min_interval: Duration::from_secs(3600),
                ..thresholds()
            },
        );

        // First scene saves immediately (no prior save to cool down from).
        state.observe_fresh(fp(&["一"], &[[0, 0, 10, 1]]), &frame(), 1);
        state.observe_stable().unwrap().unwrap();
        assert_eq!(count_captures(dir.path()), 1);

        // A second distinct scene arms but the cooldown blocks the flush; the
        // arm survives for a later attempt.
        state.observe_fresh(fp(&["二"], &[[0, 50, 10, 1]]), &frame(), 1);
        assert!(state.observe_stable().is_none());
        assert!(state.observe_stable().is_none());
        assert_eq!(count_captures(dir.path()), 1);
    }

    #[test]
    fn per_session_cap_stops_saving() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AutoCaptureState::new(
            dir.path().to_path_buf(),
            AutoCaptureThresholds {
                max_per_session: 1,
                ..thresholds()
            },
        );

        state.observe_fresh(fp(&["一"], &[[0, 0, 10, 1]]), &frame(), 1);
        state.observe_stable().unwrap().unwrap();

        // Cap reached: a second distinct scene never arms.
        assert!(
            state
                .observe_fresh(fp(&["二"], &[[0, 50, 10, 1]]), &frame(), 1)
                .is_none()
        );
        assert!(state.observe_stable().is_none());
        assert_eq!(count_captures(dir.path()), 1);
    }

    #[test]
    fn fingerprints_dedup_across_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let scene = fp(&["前回保存した画面"], &[[0, 0, 10, 1]]);

        // Session one saves the scene.
        {
            let mut state = state(dir.path());
            state.observe_fresh(scene.clone(), &frame(), 1);
            state.observe_stable().unwrap().unwrap();
        }
        assert_eq!(count_captures(dir.path()), 1);

        // Session two loads the fingerprint from disk and refuses to re-save.
        let mut next = state(dir.path());
        assert!(next.observe_fresh(scene, &frame(), 1).is_none());
        assert!(next.observe_stable().is_none());
        assert_eq!(count_captures(dir.path()), 1);
    }
}
