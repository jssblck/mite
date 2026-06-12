//! Offline OCR latency profiler.
//!
//! Loads the real ONNX engine and a captured frame, warms the GPU, then times
//! `detect` and `recognize` over many iterations and prints p50/p95/p99. A final
//! iteration runs with sub-step profiling enabled so we can see where the time
//! inside each stage actually goes (CPU preprocessing vs. GPU inference vs.
//! postprocessing).
//!
//! Usage:
//!   cargo run --release --example profile_ocr -- [image.png] [iters]
//!   cargo run --release --example profile_ocr -- --window-id <id> [iters]
//!   cargo run --release --example profile_ocr -- --title "Window title" [iters]

use std::time::Instant;

use anyhow::{Context, Result, bail};
use mite::capture::{
    Frame, FrameSource, ImageFileCapture, WindowCapturePreference, WindowSelector,
    window_frame_source,
};
use mite::config::AppConfig;
use mite::ocr::{build_ocr_engine, filter_recognized_items};

fn pct(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[idx]
}

fn summarize(label: &str, mut v: Vec<f64>) {
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    println!(
        "{label:>16}: p50 {:.1}ms  p95 {:.1}ms  p99 {:.1}ms  mean {:.1}ms  (n={})",
        pct(&v, 0.50),
        pct(&v, 0.95),
        pct(&v, 0.99),
        mean,
        v.len()
    );
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,ort=warn")),
        )
        .init();

    let args = ProfileArgs::parse()?;
    let iters = args.iters;

    let mut config = AppConfig::load("mite.toml").unwrap_or_default();
    if let Some(det) = std::env::var_os("MITE_DET") {
        config.models.detector_path = det.into();
    }
    if let Some(rec) = std::env::var_os("MITE_REC") {
        config.models.recognizer_path = rec.into();
    }
    if std::env::var_os("MITE_INT8").is_some() {
        config.runtime.int8_detector = true;
        config.runtime.int8_recognizer = true;
    }
    if std::env::var_os("MITE_INT8_DET").is_some() {
        config.runtime.int8_detector = true;
    }
    if std::env::var_os("MITE_INT8_REC").is_some() {
        config.runtime.int8_recognizer = true;
    }
    if let Some(backend) = std::env::var_os("MITE_BACKEND") {
        use mite::config::RuntimeBackend;
        config.runtime.backend = match backend.to_string_lossy().as_ref() {
            "cuda" => RuntimeBackend::Cuda,
            "trt" | "tensorrt" => RuntimeBackend::NvidiaTensorRtThenCuda,
            "fixture" => RuntimeBackend::Fixture,
            other => panic!("unknown MITE_BACKEND {other}"),
        };
    }
    let mut pipeline = config.pipeline.into_inner();
    apply_pipeline_env_overrides(&mut pipeline);
    config.pipeline = pipeline.parse()?;
    println!(
        "runtime backend: {:?}  fp16: {}",
        config.runtime.backend, config.runtime.fp16
    );
    println!(
        "detector: {}\nrecognizer: {}",
        config.models.detector_path.display(),
        config.models.recognizer_path.display()
    );
    let mut engine = build_ocr_engine(&config.runtime, &config.models)?;

    let mut source = args.source.open()?;
    let frame: Frame = source.next_frame()?;
    if let Some(path) = std::env::var_os("MITE_CAPTURE_OUT")
        && let Some(image) = frame.pixels.as_ref()
    {
        image.save(&path)?;
        println!(
            "saved capture: {}",
            std::path::PathBuf::from(path).display()
        );
    }
    println!(
        "frame: {}x{}  ({})",
        frame.size.width,
        frame.size.height,
        args.source.describe()
    );

    // Warm up the GPU (cuDNN algo selection, allocations) before timing.
    let warmup: usize = std::env::var("MITE_WARMUP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);
    for _ in 0..warmup {
        let boxes = engine.detect(&frame, &config.pipeline)?;
        let _ = engine.recognize(&frame, &boxes)?;
    }

    let boxes_once = engine.detect(&frame, &config.pipeline)?;
    let raw_items_once = engine.recognize(&frame, &boxes_once)?;
    let items_once = filter_recognized_items(raw_items_once.clone(), &config.pipeline);
    println!(
        "detected boxes: {}  recognized lines (post-filter): {}",
        boxes_once.len(),
        items_once.len()
    );
    if std::env::var_os("MITE_PRINT_RAW_TEXT").is_some() {
        let mut lines: Vec<&mite::ocr::RecognizedText> = raw_items_once.iter().collect();
        lines.sort_by(|a, b| {
            a.text_box
                .rect
                .y
                .total_cmp(&b.text_box.rect.y)
                .then(a.text_box.rect.x.total_cmp(&b.text_box.rect.x))
        });
        println!("--- raw recognized text (reading order) ---");
        for l in lines {
            println!(
                "[box {:.2} rec {:.2}] x={:.0} y={:.0} w={:.0} h={:.0} {}",
                l.text_box.confidence,
                l.confidence,
                l.text_box.rect.x,
                l.text_box.rect.y,
                l.text_box.rect.width,
                l.text_box.rect.height,
                l.text
            );
        }
        println!("--- end raw text ---");
    }
    if std::env::var_os("MITE_PRINT_TEXT").is_some() {
        let mut lines: Vec<&mite::ocr::RecognizedText> = items_once.iter().collect();
        lines.sort_by(|a, b| {
            a.text_box
                .rect
                .y
                .total_cmp(&b.text_box.rect.y)
                .then(a.text_box.rect.x.total_cmp(&b.text_box.rect.x))
        });
        println!("--- recognized text (reading order) ---");
        for l in lines {
            println!("[{:.2}] {}", l.confidence, l.text);
        }
        println!("--- end text ---");
    }

    let mut det_ms = Vec::with_capacity(iters);
    let mut rec_ms = Vec::with_capacity(iters);
    let mut total_ms = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t0 = Instant::now();
        let boxes = engine.detect(&frame, &config.pipeline)?;
        let t1 = Instant::now();
        let _ = engine.recognize(&frame, &boxes)?;
        let t2 = Instant::now();
        det_ms.push((t1 - t0).as_secs_f64() * 1000.0);
        rec_ms.push((t2 - t1).as_secs_f64() * 1000.0);
        total_ms.push((t2 - t0).as_secs_f64() * 1000.0);
    }

    println!("\n=== warm per-stage latency over {iters} iters ===");
    summarize("detect", det_ms);
    summarize("recognize", rec_ms);
    summarize("detect+recognize", total_ms);

    // One more pass with sub-step profiling on, to attribute time within stages.
    // Skipped when iters==0 (correctness-only checks) to avoid an extra heavy pass.
    if iters > 0 {
        println!("\n=== sub-step breakdown (single warm pass) ===");
        // SAFETY: single-threaded at this point; we only flip a profiling flag the
        // engine reads via an env lookup.
        unsafe {
            std::env::set_var("MITE_PROFILE", "1");
        }
        let boxes = engine.detect(&frame, &config.pipeline)?;
        let _ = engine.recognize(&frame, &boxes)?;
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct ProfileArgs {
    source: ProfileSource,
    iters: usize,
}

impl ProfileArgs {
    fn parse() -> Result<Self> {
        let mut title = None;
        let mut window_id = None;
        let mut pid = None;
        let mut backend = WindowCapturePreference::Auto;
        let mut positional = Vec::new();
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--title" => {
                    title = Some(args.next().context("--title requires a value")?);
                }
                "--window-id" => {
                    window_id = Some(
                        args.next()
                            .context("--window-id requires a value")?
                            .parse()
                            .context("--window-id must be a u32")?,
                    );
                }
                "--pid" => {
                    pid = Some(
                        args.next()
                            .context("--pid requires a value")?
                            .parse()
                            .context("--pid must be a u32")?,
                    );
                }
                "--capture-backend" => {
                    backend =
                        parse_backend(&args.next().context("--capture-backend requires a value")?)?;
                }
                other if other.starts_with("--") => bail!("unknown argument {other}"),
                other => positional.push(other.to_string()),
            }
        }

        let iters = positional
            .get(1)
            .and_then(|value| value.parse().ok())
            .or_else(|| {
                if title.is_some() || window_id.is_some() || pid.is_some() {
                    positional.first().and_then(|value| value.parse().ok())
                } else {
                    None
                }
            })
            .unwrap_or(30);

        let source = if title.is_some() || window_id.is_some() || pid.is_some() {
            if positional.len() > 1
                || positional
                    .first()
                    .is_some_and(|value| value.parse::<usize>().is_err())
            {
                bail!("window mode accepts only an optional iteration count");
            }
            ProfileSource::Window {
                selector: WindowSelector::new(title, window_id, pid)?,
                backend,
            }
        } else {
            ProfileSource::Image(
                positional
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "target/capture-reports/bench/frame.png".to_string()),
            )
        };

        Ok(Self { source, iters })
    }
}

#[derive(Debug, Clone)]
enum ProfileSource {
    Image(String),
    Window {
        selector: WindowSelector,
        backend: WindowCapturePreference,
    },
}

impl ProfileSource {
    fn open(&self) -> Result<Box<dyn FrameSource + Send>> {
        match self {
            Self::Image(path) => Ok(Box::new(ImageFileCapture::new(path)?)),
            Self::Window { selector, backend } => {
                Ok(window_frame_source(selector.clone(), *backend))
            }
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Image(path) => path.clone(),
            Self::Window { selector, backend } => {
                format!("{} via {backend:?}", selector.describe())
            }
        }
    }
}

fn parse_backend(raw: &str) -> Result<WindowCapturePreference> {
    match raw {
        "auto" => Ok(WindowCapturePreference::Auto),
        "wgc" | "windows-graphics-capture" => Ok(WindowCapturePreference::WindowsGraphicsCapture),
        "screenshot" | "window-screenshot" => Ok(WindowCapturePreference::WindowScreenshot),
        other => bail!("unknown capture backend {other}"),
    }
}

fn apply_pipeline_env_overrides(config: &mut mite::config::PipelineConfig) {
    env_f32("MITE_DETECTOR_DOWNSCALE", |value| {
        config.detector_downscale = value;
    });
    env_u32("MITE_DETECTOR_MIN_LONG_SIDE", |value| {
        config.detector_min_long_side = value;
    });
    env_u32("MITE_DETECTOR_MAX_LONG_SIDE", |value| {
        config.detector_max_long_side = value;
    });
    env_usize("MITE_MAX_BOXES_PER_FRAME", |value| {
        config.max_boxes_per_frame = value;
    });
    env_f32("MITE_DETECTOR_PROBABILITY_THRESHOLD", |value| {
        config.detector_probability_threshold = value;
    });
    env_f32("MITE_DETECTOR_BOX_SCORE_THRESHOLD", |value| {
        config.detector_box_score_threshold = value;
    });
    env_bool("MITE_DETECTOR_LOW_CONTRAST_PASS", |value| {
        config.detector_low_contrast_pass = value;
    });
    env_f32(
        "MITE_DETECTOR_LOW_CONTRAST_PROBABILITY_THRESHOLD",
        |value| {
            config.detector_low_contrast_probability_threshold = value;
        },
    );
    env_f32("MITE_DETECTOR_LOW_CONTRAST_BOX_SCORE_THRESHOLD", |value| {
        config.detector_low_contrast_box_score_threshold = value;
    });
    env_usize("MITE_DETECTOR_MIN_COMPONENT_AREA", |value| {
        config.detector_min_component_area = value;
    });
    env_f32("MITE_MIN_RECOGNITION_CONFIDENCE", |value| {
        config.min_recognition_confidence = value;
    });
    env_bool("MITE_DETECTOR_CONTRAST_STRETCH", |value| {
        config.detector_contrast_stretch = value;
    });
}

fn env_f32(name: &str, apply: impl FnOnce(f32)) {
    if let Ok(raw) = std::env::var(name) {
        apply(
            raw.parse()
                .unwrap_or_else(|_| panic!("{name} must be a float")),
        );
    }
}

fn env_u32(name: &str, apply: impl FnOnce(u32)) {
    if let Ok(raw) = std::env::var(name) {
        apply(
            raw.parse()
                .unwrap_or_else(|_| panic!("{name} must be a u32")),
        );
    }
}

fn env_usize(name: &str, apply: impl FnOnce(usize)) {
    if let Ok(raw) = std::env::var(name) {
        apply(
            raw.parse()
                .unwrap_or_else(|_| panic!("{name} must be a usize")),
        );
    }
}

fn env_bool(name: &str, apply: impl FnOnce(bool)) {
    if let Ok(raw) = std::env::var(name) {
        apply(matches!(
            raw.as_str(),
            "1" | "true" | "TRUE" | "yes" | "YES"
        ));
    }
}
