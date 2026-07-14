//! `mite watch`: a persistent, yomichan-style OCR overlay.
//!
//! While the **Shift** key is held, the foreground window is captured and OCR'd
//! and each recognized word is drawn with a POS-coloured underline (unless
//! `overlay.word_underlines` is off, and, when `overlay.furigana` is enabled,
//! furigana above it). Hovering the cursor over a word draws a definition popup
//! regardless of those settings.
//! Releasing Shift clears the overlay; **Esc** quits.
//!
//! Some games intercept Shift while focused, so the trigger never fires. For
//! those, `--auto` runs the capture/OCR loop continuously with no key held
//! (best paired with `--window-id`/`--title` to pin the game window).
//!
//! `--focus-only` (which requires a pinned window) gates all drawing on focus:
//! the overlay presents only while the pinned window is foreground, so pinned
//! ink never lingers over whatever the user alt-tabs to, and OCR pauses while
//! the target is unfocused (unless `--auto-eval-capture` keeps the loop running
//! for background collection; presentation stays gated either way).
//!
//! `--auto-eval-capture` also drives the loop continuously (it observes OCR
//! passes to detect scene changes), so it collects fixtures hands-free without
//! Shift or `--auto`. Presentation stays gated on Shift/`--auto`, so background
//! collection draws no overlay.
//!
//! The overlay window stays click-through, so the game keeps all input. We
//! read the cursor position by polling rather than capturing mouse events.
//!
//! Capture + OCR run on a background worker thread. The UI thread (overlay
//! message pump, input polling, hover → popup) stays responsive at ~60 Hz and
//! simply applies OCR snapshots as the worker produces them, so a slow OCR pass
//! never freezes hovering.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_ESCAPE, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOTOWNER, GetAncestor, GetCursorPos, GetForegroundWindow,
};

use crate::capture::{
    Frame, FrameDelivery, FrameSource, WindowCapturePreference, WindowSelector, window_frame_source,
};
use crate::config::AppConfig;
use crate::dictionary::Dictionary;
use crate::eval_capture;
use crate::geometry::{Rect, ScreenRect};
use crate::hotkey::{GlobalHotkey, HotkeyCombo};
use crate::hover::{
    Highlight, WordSpan, build_word_spans_from_line_tokens, cursor_to_local, hit_test,
};
use crate::hud::{PassCounts, PassExtras, stage_clock};
use crate::ocr::{OcrEngine, RecognizedText, build_ocr_engine, filter_recognized_items};
use crate::text_blocks::{analyze_recognized_lines, sort_recognized_reading_order};
use crate::win32_overlay::{OverlayEvent, Popup, Win32Overlay};

mod auto_capture;
mod smoothing;

pub use auto_capture::AutoCaptureThresholds;
use auto_capture::{AutoCaptureState, fingerprint_from_items};
use smoothing::{Anchor, CachedDetection, SmoothingState};

/// Tunables for the interactive watch loop.
#[derive(Debug, Clone)]
pub struct WatchRequest {
    pub lexicon: PathBuf,
    /// Minimum delay between OCR passes while Shift is held.
    pub refresh: Duration,
    pub max_senses: usize,
    pub max_glosses: usize,
    /// When set, always capture this window instead of following the foreground
    /// window. Useful for 3D games (whose foreground HWND can be awkward) and
    /// for deterministic debugging.
    pub pinned_window_id: Option<u32>,
    /// Capture backend preference passed to the worker.
    pub backend: WindowCapturePreference,
    /// Run continuously without holding Shift (for games that intercept Shift).
    pub auto: bool,
    /// Present the overlay only while the pinned target window is focused (it,
    /// or a window it owns, is the foreground window). Alt-tabbing away clears
    /// the overlay within one UI tick, including a sticky hover popup, and OCR
    /// pauses until the target regains focus (unless `auto_eval_capture` keeps
    /// the loop running for background fixture collection; presentation stays
    /// gated either way). Meaningful only with `pinned_window_id`: without one
    /// the gate fails closed and nothing ever draws, so the CLI rejects that
    /// combination up front.
    pub focus_only: bool,
    /// Draw the per-stage latency HUD (graph + p50/p95/p99) in the top-left.
    pub hud: bool,
    /// If non-zero, log an aggregated per-stage timing report to stderr on this
    /// interval (headless equivalent of the HUD).
    pub metrics_interval: Duration,
    /// Reuse the previous detection when the scene is essentially unchanged
    /// (skips detect+recognize+analyze), for lower latency and stable overlays.
    pub smoothing: bool,
    /// Optional developer-only hotkey that saves raw frames for eval fixtures
    /// without running OCR.
    pub eval_hotkey: Option<EvalHotkeyRequest>,
    /// Optional automatic eval-fixture capture: save a raw frame whenever the
    /// detected text or box layout changes enough to be a new scene.
    pub auto_eval_capture: Option<AutoEvalCaptureRequest>,
}

#[derive(Debug, Clone)]
pub struct EvalHotkeyRequest {
    pub combo: HotkeyCombo,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AutoEvalCaptureRequest {
    pub output_dir: PathBuf,
    pub thresholds: AutoCaptureThresholds,
}

/// One captured-and-analyzed snapshot of the target window: the segmented words
/// (highlight geometry + popup content) in frame-local coordinates, plus the raw
/// OCR lines (retained for the HUD's line count).
struct Snapshot {
    screen_rect: ScreenRect,
    items: Vec<RecognizedText>,
    words: Vec<WordSpan>,
    /// Capture-health extras for the HUD/metrics (drop count + staleness).
    extras: PassExtras,
}

/// Per-pass worker settings: how much of each entry the popup shows, and which
/// capture backend to use.
#[derive(Debug, Clone, Copy)]
struct Limits {
    max_senses: usize,
    max_glosses: usize,
    backend: WindowCapturePreference,
    smoothing: bool,
}

/// Re-detect at least this often even on a "stable" scene, to self-correct from
/// any missed change.
const MAX_REUSE: Duration = Duration::from_secs(3);

/// How long the UI loop sleeps between iterations (~60 Hz): pumps Win32
/// messages, polls input, and applies OCR snapshots without busy-spinning.
const UI_POLL_INTERVAL: Duration = Duration::from_millis(16);

/// Mask for the high-order bit of `GetAsyncKeyState`, set while a key is down.
const KEY_DOWN_MASK: u16 = 0x8000;
const EVAL_CAPTURE_HOTKEY_ID: i32 = 0x4D54; // "MT", inside the app-defined range.

pub fn run_watch(config: &AppConfig, request: &WatchRequest) -> Result<()> {
    enable_dpi_awareness();

    let dict = Dictionary::load(&request.lexicon)?;
    println!(
        "loaded {} dictionary entries from {}",
        dict.entry_count(),
        request.lexicon.display()
    );
    // Build the engine up front so model/load errors surface before the loop.
    let engine = build_ocr_engine(&config.runtime, &config.models)?;
    let mut overlay = Win32Overlay::new()?;
    if request.hud {
        overlay.enable_hud();
    }
    overlay.set_furigana_visible(config.overlay.furigana);
    overlay.set_underlines_visible(config.overlay.word_underlines);

    println!(
        "watching: hold SHIFT over a window to OCR it, hover a word to look it up, ESC to quit."
    );

    match request.pinned_window_id {
        Some(id) => println!(
            "target: pinned window id {id} (backend {:?})",
            request.backend
        ),
        None => println!("target: foreground window (backend {:?})", request.backend),
    }
    if request.auto {
        println!("mode: --auto (running continuously; no Shift needed). ESC to quit.");
    }
    if request.focus_only {
        println!("mode: --focus-only (overlay drawn only while the target window is focused).");
    }
    let eval_hotkey_registration = match &request.eval_hotkey {
        Some(eval) => {
            let registration = GlobalHotkey::register(EVAL_CAPTURE_HOTKEY_ID, eval.combo.clone())?;
            println!(
                "eval capture hotkey: {} -> {} (raw frame, no OCR)",
                eval.combo,
                eval.output_dir.display()
            );
            Some(registration)
        }
        None => None,
    };
    if let Some(auto) = &request.auto_eval_capture {
        println!(
            "auto eval capture: on -> {} (saves a raw frame when the scene changes)",
            auto.output_dir.display()
        );
    }

    let limits = Limits {
        max_senses: request.max_senses,
        max_glosses: request.max_glosses,
        backend: request.backend,
        smoothing: request.smoothing,
    };
    let (job_tx, worker_rx) = spawn_watch_worker(
        engine,
        dict,
        config.clone(),
        limits,
        request.auto_eval_capture.clone(),
    );

    let mut snapshot: Option<Snapshot> = None;
    let mut in_flight = false;
    let mut last_request: Option<Instant> = None;
    let mut last_window: Option<u32> = None;
    let mut showing = false;
    let mut last_metrics = Instant::now();

    loop {
        let overlay_events = overlay.pump();
        if key_down(VK_ESCAPE.0) {
            break;
        }

        // Periodic headless metrics dump (same data as the HUD), for measuring
        // the pipeline without the GUI.
        if !request.metrics_interval.is_zero()
            && last_metrics.elapsed() >= request.metrics_interval
            && !overlay.hud().is_empty()
        {
            tracing::info!("{}", overlay.hud().report());
            last_metrics = Instant::now();
        }
        // `--auto` makes OCR run continuously (no Shift). Otherwise Shift is the
        // hold-to-activate trigger, but games often intercept it while focused.
        // `--focus-only` additionally gates everything on the pinned target
        // being focused, checked every UI tick so alt-tabbing away clears the
        // overlay within one poll interval. The overlay window itself is
        // WS_EX_NOACTIVATE and can never become the foreground window, so
        // hovering the popup does not defeat the gate.
        let focus_allows = !request.focus_only
            || request
                .pinned_window_id
                .is_some_and(|id| target_focused(id, foreground_window()));
        let active = (request.auto || key_down(VK_SHIFT.0)) && focus_allows;

        if eval_hotkey_registration
            .as_ref()
            .is_some_and(|registration| {
                overlay_events.iter().any(
                    |event| matches!(event, OverlayEvent::Hotkey(id) if *id == registration.id()),
                )
            })
            && let Some(eval) = &request.eval_hotkey
        {
            if let Some(window_id) = request.pinned_window_id.or_else(foreground_window_id) {
                let _ = job_tx.send(WorkerJob::EvalCapture {
                    window_id,
                    output_dir: eval.output_dir.clone(),
                });
            } else {
                tracing::warn!("eval hotkey pressed, but no foreground window was available");
            }
        }

        // Apply any OCR results the worker finished. A `None` still clears the
        // in-flight flag so a failed pass does not wedge the request gate.
        while let Ok(event) = worker_rx.try_recv() {
            match event {
                WorkerEvent::OcrPass(result) => {
                    in_flight = false;
                    if let Some(snap) = result
                        && active
                    {
                        let highlights: Vec<Highlight> =
                            snap.words.iter().map(WordSpan::highlight).collect();
                        // Content counts surface in the HUD.
                        let counts = PassCounts {
                            lines: snap.items.len(),
                            words: highlights.len(),
                            known: snap.words.iter().filter(|w| w.known).count(),
                        };
                        {
                            // Time compositing under a `present` span so it lands in the
                            // same StageClock the worker stages feed.
                            let _span = tracing::info_span!("present").entered();
                            overlay.present_snapshot(snap.screen_rect, &highlights);
                        }
                        // The `present` span has now closed, so this pass's full
                        // capture→present timings are current. Always record (the HUD and
                        // the headless metrics dump both read this window).
                        overlay.record_pass(stage_clock().snapshot(), counts, snap.extras);
                        snapshot = Some(snap);
                        showing = true;
                    }
                }
                WorkerEvent::EvalCapture(Ok(dir)) => {
                    println!("eval capture saved: {}", dir.display());
                }
                WorkerEvent::EvalCapture(Err(error)) => {
                    tracing::warn!("eval capture failed: {error}");
                }
            }
        }

        // Locate the cursor relative to the current snapshot's interactive
        // regions (word highlights and the popup panel).
        let cursor = cursor_position();
        let local = snapshot
            .as_ref()
            .zip(cursor)
            .map(|(snap, (cx, cy))| cursor_to_local(cx, cy, snap.screen_rect));
        let over_highlight = matches!((&snapshot, local), (Some(snap), Some((x, y)))
            if snap.words.iter().any(|word| word.rect.contains(x, y)));
        let over_popup = matches!((overlay.popup_panel(), local), (Some(panel), Some((x, y)))
            if panel.contains(x, y));
        let over_interactive = over_highlight || over_popup;

        // Auto eval capture observes OCR passes, so the loop must keep running
        // them even when the overlay is not active (no Shift, no --auto):
        // otherwise the documented hands-free collection would capture nothing.
        // Presentation and hover stay gated on `active` below, so background
        // collection does not force the overlay on.
        let ocr_running = active || request.auto_eval_capture.is_some();

        if ocr_running {
            // Request a fresh OCR pass when the target window changes or the
            // refresh interval elapses, but freeze while hovering content so
            // the popup you are reading isn't reset under you. (`over_interactive`
            // is only ever true while the overlay is active and showing.)
            if let Some(window_id) = request.pinned_window_id.or_else(foreground_window_id) {
                let target_changed = Some(window_id) != last_window;
                let due = last_request.is_none_or(|at| at.elapsed() >= request.refresh);
                if !in_flight && !over_interactive && (target_changed || due) {
                    let _ = job_tx.send(WorkerJob::OcrPass(window_id));
                    in_flight = true;
                    last_request = Some(Instant::now());
                    last_window = Some(window_id);
                }
            }
        }

        if active {
            if !over_popup {
                update_hover(&mut overlay, snapshot.as_ref(), local);
            }
        } else if showing && over_interactive && focus_allows {
            // Sticky: keep the overlay alive while the cursor is over a word or
            // the popup, even after Shift is released, so the popup stays
            // readable. The focus gate overrides stickiness: a popup pinned to
            // an unfocused window would hang over whatever is focused instead.
            if !over_popup {
                update_hover(&mut overlay, snapshot.as_ref(), local);
            }
        } else if showing {
            overlay.clear();
            snapshot = None;
            showing = false;
            last_request = None;
            last_window = None;
        }

        thread::sleep(UI_POLL_INTERVAL);
    }

    Ok(())
}

enum WorkerJob {
    OcrPass(u32),
    EvalCapture { window_id: u32, output_dir: PathBuf },
}

enum WorkerEvent {
    OcrPass(Option<Snapshot>),
    EvalCapture(std::result::Result<PathBuf, String>),
}

/// Spawn the worker that owns the OCR engine, dictionary, and capture session.
/// It receives OCR jobs and raw eval-capture jobs, then replies with either an
/// analyzed snapshot or the saved raw-capture folder.
fn spawn_watch_worker(
    engine: Box<dyn OcrEngine + Send>,
    dict: Dictionary,
    config: AppConfig,
    limits: Limits,
    auto_eval_capture: Option<AutoEvalCaptureRequest>,
) -> (Sender<WorkerJob>, Receiver<WorkerEvent>) {
    let (job_tx, job_rx) = mpsc::channel::<WorkerJob>();
    let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();

    thread::spawn(move || {
        let mut worker = Worker::new(engine, dict, config, limits, auto_eval_capture);
        while let Ok(job) = job_rx.recv() {
            let event = match job {
                WorkerJob::OcrPass(window_id) => {
                    let result = match worker.pass(window_id) {
                        Ok(snapshot) => Some(snapshot),
                        Err(error) => {
                            tracing::warn!("OCR pass failed for window {window_id}: {error:#}");
                            None
                        }
                    };
                    // An auto-capture may have fired during the pass; surface it
                    // with the same event the manual hotkey uses.
                    if let Some(saved) = worker.take_auto_capture_result()
                        && event_tx.send(WorkerEvent::EvalCapture(saved)).is_err()
                    {
                        break;
                    }
                    WorkerEvent::OcrPass(result)
                }
                WorkerJob::EvalCapture {
                    window_id,
                    output_dir,
                } => {
                    let result = worker
                        .raw_eval_capture(window_id, &output_dir)
                        .map_err(|error| format!("{error:#}"));
                    WorkerEvent::EvalCapture(result)
                }
            };
            if event_tx.send(event).is_err() {
                break; // UI thread has exited.
            }
        }
    });

    (job_tx, event_rx)
}

/// The OCR worker's long-lived state, driven once per requested job by the
/// background thread in [`spawn_watch_worker`].
struct Worker {
    engine: Box<dyn OcrEngine + Send>,
    dict: Dictionary,
    config: AppConfig,
    limits: Limits,
    /// Active capture source and the window id it targets; rebuilt when the
    /// requested window changes.
    capture: Option<(u32, Box<dyn FrameSource + Send>)>,
    last_id: Option<u32>,
    smoothing: SmoothingState,
    /// Automatic eval-fixture capture state, when `--auto-eval-capture` is on.
    auto_capture: Option<AutoCaptureState>,
    /// Result of any auto-capture that fired during the current pass, drained by
    /// the worker loop after `pass` returns.
    auto_capture_result: Option<std::result::Result<PathBuf, String>>,
}

impl Worker {
    fn new(
        engine: Box<dyn OcrEngine + Send>,
        dict: Dictionary,
        config: AppConfig,
        limits: Limits,
        auto_eval_capture: Option<AutoEvalCaptureRequest>,
    ) -> Self {
        let auto_capture = auto_eval_capture
            .map(|request| AutoCaptureState::new(request.output_dir, request.thresholds));
        Self {
            engine,
            dict,
            config,
            limits,
            capture: None,
            last_id: None,
            smoothing: SmoothingState::new(),
            auto_capture,
            auto_capture_result: None,
        }
    }

    fn take_auto_capture_result(&mut self) -> Option<std::result::Result<PathBuf, String>> {
        self.auto_capture_result.take()
    }

    /// Capture the target window, run detection + recognition, then segment each
    /// recognized line into word spans (highlight geometry + popup content).
    fn pass(&mut self, window_id: u32) -> Result<Snapshot> {
        // Cleared each pass so a stale auto-capture result is never re-sent.
        self.auto_capture_result = None;
        let target_changed = self.prepare_capture(window_id)?;

        // Temporal smoothing: if the previously detected text regions are
        // unchanged (sampled in the new frame at the same rects), reuse that
        // detection and skip the expensive stages. Comparing only text regions
        // ignores animated game backgrounds. Bounded by MAX_REUSE so any missed
        // change self-corrects. When eligible, the anchor is handed to the
        // capture source as a probe so a WGC source can answer "unchanged"
        // straight off the staging buffer, skipping frame materialization
        // entirely; sources without that vantage return a full frame and the
        // same signature is checked here instead.
        let probe_eligible = self.limits.smoothing
            && !target_changed
            && self.smoothing.last_full.elapsed() < MAX_REUSE
            && self.smoothing.cached.is_some();
        let frame_probe = self
            .smoothing
            .anchor
            .as_ref()
            .map(|anchor| anchor.probe().clone());
        let source = self.capture_source_mut()?;
        let delivery = {
            let _span = tracing::info_span!("capture").entered();
            match (probe_eligible, frame_probe.as_ref()) {
                (true, Some(anchor)) => source
                    .next_frame_or_unchanged(anchor)
                    .context("failed to capture frame")?,
                _ => FrameDelivery::Frame(source.next_frame().context("failed to capture frame")?),
            }
        };
        let frame = match delivery {
            FrameDelivery::Unchanged(unchanged) => {
                let cached = self
                    .smoothing
                    .cached
                    .clone()
                    .context("probe reuse was selected without cached detection")?;
                // Enter the skipped stages as empty spans so the metrics/HUD
                // reflect the reuse (≈0 ms) instead of carrying stale durations.
                drop(tracing::info_span!("detect").entered());
                drop(tracing::info_span!("recognize").entered());
                drop(tracing::info_span!("analyze").entered());
                self.observe_stable_for_auto_capture();
                return Ok(Snapshot {
                    screen_rect: unchanged.screen_rect,
                    items: cached.items,
                    words: cached.words,
                    extras: PassExtras {
                        staging_age: unchanged.staging_age,
                        frames_delivered: unchanged.frames_delivered,
                    },
                });
            }
            FrameDelivery::Frame(frame) => frame,
        };
        let screen_rect = frame.screen_rect;
        let extras = PassExtras {
            staging_age: frame.staging_age,
            frames_delivered: frame.frames_delivered,
        };

        let reuse = probe_eligible
            && match (&self.smoothing.anchor, frame.pixels.as_deref()) {
                (Some(anchor), Some(image)) => anchor.matches(image),
                _ => false,
            };
        if reuse && let Some(cached) = self.smoothing.cached.clone() {
            // Enter the skipped stages as empty spans so the metrics/HUD reflect
            // the reuse (≈0 ms) instead of carrying stale durations.
            drop(tracing::info_span!("detect").entered());
            drop(tracing::info_span!("recognize").entered());
            drop(tracing::info_span!("analyze").entered());
            self.observe_stable_for_auto_capture();
            return Ok(Snapshot {
                screen_rect,
                items: cached.items,
                words: cached.words,
                extras,
            });
        }

        let boxes = {
            let _span = tracing::info_span!("detect").entered();
            self.engine.detect(&frame, &self.config.pipeline)?
        };
        let mut items = {
            let _span = tracing::info_span!("recognize").entered();
            filter_recognized_items(
                self.engine.recognize(&frame, &boxes)?,
                &self.config.pipeline,
            )
        };
        sort_recognized_reading_order(&mut items);

        tracing::debug!(
            "OCR pass: window={window_id} via {} {}x{} @ ({},{}) → {} line(s)",
            frame.source.kind.as_str(),
            screen_rect.size.width,
            screen_rect.size.height,
            screen_rect.x,
            screen_rect.y,
            items.len(),
        );

        let words = {
            let _span = tracing::info_span!("analyze").entered();
            let mut words = Vec::new();
            for line in analyze_recognized_lines(&self.dict, &items) {
                words.extend(build_word_spans_from_line_tokens(
                    line.item.text_box.rect,
                    &line.item.text,
                    &line.tokens,
                    &line.block_tokens,
                    &line.item.char_centers,
                    self.limits.max_senses,
                    self.limits.max_glosses,
                ));
            }
            words
        };

        // Anchor this full detection for subsequent stable-scene reuse: remember
        // the detected text rects and their luma so the next frame can be
        // compared there.
        let rects: Vec<Rect> = items.iter().map(|item| item.text_box.rect).collect();
        self.smoothing.anchor = frame
            .pixels
            .as_ref()
            .map(|img| Anchor::from_detection(img, rects));
        self.smoothing.cached = Some(CachedDetection::new(items.clone(), words.clone()));
        self.smoothing.last_full = Instant::now();

        // Offer this fresh detection to the auto-capture decision. It arms on a
        // significant change and flushes once the scene settles; the exact frame
        // that produced the detection is what gets saved.
        self.observe_fresh_for_auto_capture(&items, &frame, window_id);

        Ok(Snapshot {
            screen_rect,
            items,
            words,
            extras,
        })
    }

    /// Feed a settled (reused) pass to the auto-capture state, recording any
    /// flushed capture for the worker loop to surface.
    fn observe_stable_for_auto_capture(&mut self) {
        if let Some(state) = self.auto_capture.as_mut() {
            self.auto_capture_result = state.observe_stable();
        }
    }

    /// Feed a fresh full detection (and the frame that produced it) to the
    /// auto-capture state. Skipped when the frame did not retain pixels, since
    /// there would be nothing to save.
    fn observe_fresh_for_auto_capture(
        &mut self,
        items: &[RecognizedText],
        frame: &Frame,
        window_id: u32,
    ) {
        let result = self.auto_capture.as_mut().and_then(|state| {
            frame.pixels.as_ref()?;
            let fingerprint = fingerprint_from_items(items);
            state.observe_fresh(fingerprint, frame, window_id)
        });
        if result.is_some() {
            self.auto_capture_result = result;
        }
    }

    fn raw_eval_capture(
        &mut self,
        window_id: u32,
        output_dir: &std::path::Path,
    ) -> Result<PathBuf> {
        let (frame, _) = self.capture_frame(window_id)?;
        eval_capture::write_raw_capture(output_dir, window_id, &frame, None)
    }

    /// Ensure a capture source exists for `window_id`; returns whether the
    /// target changed since the previous pass.
    fn prepare_capture(&mut self, window_id: u32) -> Result<bool> {
        let target_changed = Some(window_id) != self.last_id;
        self.last_id = Some(window_id);

        if target_changed || self.capture.is_none() {
            let selector = WindowSelector::new(None, Some(window_id), None)?;
            let source = window_frame_source(selector, self.limits.backend);
            self.capture = Some((window_id, source));
        }
        Ok(target_changed)
    }

    fn capture_frame(&mut self, window_id: u32) -> Result<(Frame, bool)> {
        let target_changed = self.prepare_capture(window_id)?;
        let source = self.capture_source_mut()?;
        // Each stage is wrapped in a `tracing` span named after the stage; the
        // `StageTimingLayer` times them and feeds the watch HUD's latency graph
        // without any timing values being threaded through these calls.
        let frame = {
            let _span = tracing::info_span!("capture").entered();
            source.next_frame().context("failed to capture frame")?
        };
        Ok((frame, target_changed))
    }

    fn capture_source_mut(&mut self) -> Result<&mut Box<dyn FrameSource + Send>> {
        self.capture
            .as_mut()
            .map(|(_, source)| source)
            .context("capture source was not initialized")
    }
}

/// Resolve the word under the cursor and update the overlay's highlight/popup.
fn update_hover(
    overlay: &mut Win32Overlay,
    snapshot: Option<&Snapshot>,
    local: Option<(f32, f32)>,
) {
    let (Some(snapshot), Some((local_x, local_y))) = (snapshot, local) else {
        overlay.set_interaction(None, None);
        return;
    };

    let rects: Vec<Rect> = snapshot.words.iter().map(|word| word.rect).collect();
    let Some(index) = hit_test(&rects, local_x, local_y) else {
        overlay.set_interaction(None, None);
        return;
    };

    let word = &snapshot.words[index];
    let popup = word.known.then(|| Popup {
        word_rect: word.rect,
        content: word.content.clone(),
    });
    overlay.set_interaction(Some(index), popup);
}

fn enable_dpi_awareness() {
    // Match capture pixels (physical) with cursor coordinates. Best-effort: if
    // awareness was already set by the manifest this simply fails harmlessly.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn key_down(vk: u16) -> bool {
    // High-order bit of GetAsyncKeyState marks the key as currently down.
    (unsafe { GetAsyncKeyState(vk as i32) } as u16 & KEY_DOWN_MASK) != 0
}

/// The current foreground window's identity: the HWND itself plus its
/// root-owner ancestor. An owned window (a game's file dialog, for example)
/// reports the game as its root owner, so focus on it still counts as the
/// target being focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ForegroundWindow {
    id: u32,
    root_owner_id: u32,
}

fn foreground_window() -> Option<ForegroundWindow> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let id = hwnd.0 as usize as u32;
    let root_owner = unsafe { GetAncestor(hwnd, GA_ROOTOWNER) };
    let root_owner_id = if root_owner.0.is_null() {
        id
    } else {
        root_owner.0 as usize as u32
    };
    Some(ForegroundWindow { id, root_owner_id })
}

fn foreground_window_id() -> Option<u32> {
    foreground_window().map(|window| window.id)
}

/// Pure focus-gate decision for `--focus-only`: the target counts as focused
/// when the foreground window is the target itself or is owned by it. No
/// foreground window at all (mid desktop switch, a secure-desktop prompt)
/// fails closed.
fn target_focused(target_id: u32, foreground: Option<ForegroundWindow>) -> bool {
    foreground.is_some_and(|window| window.id == target_id || window.root_owner_id == target_id)
}

fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_ok() {
        Some((point.x, point.y))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: u32 = 0x1000;

    #[test]
    fn target_focused_when_foreground_is_the_target() {
        let foreground = ForegroundWindow {
            id: TARGET,
            root_owner_id: TARGET,
        };
        assert!(target_focused(TARGET, Some(foreground)));
    }

    #[test]
    fn target_focused_when_foreground_is_owned_by_the_target() {
        // A dialog owned by the target (its root owner) keeps the gate open.
        let dialog = ForegroundWindow {
            id: 0x2000,
            root_owner_id: TARGET,
        };
        assert!(target_focused(TARGET, Some(dialog)));
    }

    #[test]
    fn target_not_focused_for_unrelated_or_missing_foreground() {
        let other = ForegroundWindow {
            id: 0x3000,
            root_owner_id: 0x3000,
        };
        assert!(!target_focused(TARGET, Some(other)));
        // No foreground window at all fails closed.
        assert!(!target_focused(TARGET, None));
    }
}
