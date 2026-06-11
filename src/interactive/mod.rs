//! `mite watch`: a persistent, yomichan-style OCR overlay.
//!
//! While the **Shift** key is held, the foreground window is captured and OCR'd
//! and each recognized word is drawn as a translucent, POS-coloured highlight.
//! Hovering the cursor over a word draws a definition popup (with furigana).
//! Releasing Shift clears the overlay; **Esc** quits.
//!
//! Some games intercept Shift while focused, so the trigger never fires. For
//! those, `--auto` runs the capture/OCR loop continuously with no key held
//! (best paired with `--window-id`/`--title` to pin the game window).
//!
//! The overlay window stays click-through, so the game keeps all input — we
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
use image::RgbImage;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_ESCAPE, VK_LBUTTON, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

use crate::capture::{
    Frame, FrameDelivery, FrameSource, WindowCapturePreference, WindowSelector, window_frame_source,
};
use crate::config::AppConfig;
use crate::debug_capture::{self, CaptureInput};
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

mod smoothing;

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
}

#[derive(Debug, Clone)]
pub struct EvalHotkeyRequest {
    pub combo: HotkeyCombo,
    pub output_dir: PathBuf,
}

/// One captured-and-analyzed snapshot of the target window: the segmented words
/// (highlight geometry + popup content) in frame-local coordinates, plus the
/// raw OCR lines and the frame image, retained for debug captures.
struct Snapshot {
    screen_rect: ScreenRect,
    window_id: u32,
    image: Option<std::sync::Arc<RgbImage>>,
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

    let limits = Limits {
        max_senses: request.max_senses,
        max_glosses: request.max_glosses,
        backend: request.backend,
        smoothing: request.smoothing,
    };
    let (job_tx, worker_rx) = spawn_watch_worker(engine, dict, config.clone(), limits);

    let mut snapshot: Option<Snapshot> = None;
    let mut in_flight = false;
    let mut last_request: Option<Instant> = None;
    let mut last_window: Option<u32> = None;
    let mut showing = false;
    let mut lbutton_was_down = false;
    let mut report_click_latched = false;
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
        // hold-to-activate trigger — but games often intercept it while focused.
        let active = request.auto || key_down(VK_SHIFT.0);

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
        let report_button = overlay.screenshot_button();
        let over_button = matches!((report_button, local), (Some(button), Some((x, y)))
            if button.contains(x, y));

        if active {
            // Request a fresh OCR pass when the target window changes or the
            // refresh interval elapses — but freeze while hovering content so
            // the popup you are reading isn't reset under you.
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
            if !over_popup {
                update_hover(&mut overlay, snapshot.as_ref(), local);
            }
        } else if showing && over_interactive {
            // Sticky: keep the overlay alive while the cursor is over a word or
            // the popup, even after Shift is released (so it can be screenshot).
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

        // Problem-report button: a left-click over the button writes a debug
        // capture. Use both overlay mouse messages and a global polling fallback:
        // the former is the reliable path once the overlay receives the click,
        // while the latter still catches fast clicks during the transparent →
        // interactive style transition.
        let lbutton_down = key_down(VK_LBUTTON.0);
        if !lbutton_down {
            report_click_latched = false;
        }
        let report_requested_by_message = overlay_events.iter().any(|event| {
            matches!(
                event,
                OverlayEvent::LeftButtonDown { x, y }
                    if button_contains(report_button, *x, *y)
            )
        });
        let report_requested_by_poll = lbutton_down && !lbutton_was_down && over_button;
        if (report_requested_by_message || report_requested_by_poll)
            && !report_click_latched
            && let Some(snap) = &snapshot
        {
            save_debug_capture(&overlay, snap);
            report_click_latched = true;
        }
        lbutton_was_down = lbutton_down;

        // Drop click-through only while over (or actively clicking) the report
        // button, so the rest of the overlay stays transparent to the game.
        overlay.set_click_through(!(over_button || report_click_latched));

        thread::sleep(UI_POLL_INTERVAL);
    }

    Ok(())
}

/// Composite the overlay over the captured frame and write a debug capture.
fn save_debug_capture(overlay: &Win32Overlay, snapshot: &Snapshot) {
    let Some(image) = &snapshot.image else {
        tracing::warn!("no frame image retained; cannot write debug capture");
        return;
    };
    let Some((width, height, bgra)) = overlay.overlay_surface() else {
        return;
    };
    let input = CaptureInput {
        frame: image,
        overlay_width: width,
        overlay_height: height,
        overlay_bgra: &bgra,
        screen_rect: snapshot.screen_rect,
        window_id: snapshot.window_id,
        lines: &snapshot.items,
        words: &snapshot.words,
    };
    match debug_capture::write_capture(&input) {
        Ok(dir) => println!("problem report capture saved: {}", dir.display()),
        Err(error) => tracing::warn!("problem report capture failed: {error:#}"),
    }
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
) -> (Sender<WorkerJob>, Receiver<WorkerEvent>) {
    let (job_tx, job_rx) = mpsc::channel::<WorkerJob>();
    let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>();

    thread::spawn(move || {
        let mut worker = Worker::new(engine, dict, config, limits);
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
}

impl Worker {
    fn new(
        engine: Box<dyn OcrEngine + Send>,
        dict: Dictionary,
        config: AppConfig,
        limits: Limits,
    ) -> Self {
        Self {
            engine,
            dict,
            config,
            limits,
            capture: None,
            last_id: None,
            smoothing: SmoothingState::new(),
        }
    }

    /// Capture the target window, run detection + recognition, then segment each
    /// recognized line into word spans (highlight geometry + popup content).
    fn pass(&mut self, window_id: u32) -> Result<Snapshot> {
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
                return Ok(Snapshot {
                    screen_rect: unchanged.screen_rect,
                    window_id,
                    image: None,
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
            return Ok(Snapshot {
                screen_rect,
                window_id,
                image: frame.pixels,
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

        let image = frame.pixels; // retained for debug captures

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
        self.smoothing.anchor = image.as_ref().map(|img| Anchor::from_detection(img, rects));
        self.smoothing.cached = Some(CachedDetection::new(items.clone(), words.clone()));
        self.smoothing.last_full = Instant::now();

        Ok(Snapshot {
            screen_rect,
            window_id,
            image,
            items,
            words,
            extras,
        })
    }

    fn raw_eval_capture(
        &mut self,
        window_id: u32,
        output_dir: &std::path::Path,
    ) -> Result<PathBuf> {
        let (frame, _) = self.capture_frame(window_id)?;
        eval_capture::write_raw_capture(output_dir, window_id, &frame)
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
        anchor_x: word.rect.x.round() as i32,
        anchor_y: word.rect.bottom().round() as i32,
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

fn foreground_window_id() -> Option<u32> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    Some(hwnd.0 as usize as u32)
}

fn cursor_position() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_ok() {
        Some((point.x, point.y))
    } else {
        None
    }
}

fn button_contains(button: Option<Rect>, x: i32, y: i32) -> bool {
    button.is_some_and(|button| button.contains(x as f32, y as f32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_contains_uses_frame_local_mouse_coordinates() {
        let button = Some(Rect::new(10.0, 20.0, 30.0, 15.0));

        assert!(button_contains(button, 10, 20));
        assert!(button_contains(button, 39, 34));
        assert!(!button_contains(button, 40, 34));
        assert!(!button_contains(button, 39, 35));
        assert!(!button_contains(None, 10, 20));
    }
}
