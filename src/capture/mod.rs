use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::{DynamicImage, RgbImage};

use crate::geometry::{ScreenRect, Size};

mod image_probe;
mod window;

pub use window::{WindowInfo, WindowSelector, list_capturable_windows};

use image_probe::{fingerprint_rgb, image_has_signal};
use window::find_window;

#[derive(Debug, Clone)]
pub struct Frame {
    pub id: u64,
    pub captured_at: Instant,
    pub size: Size,
    pub screen_rect: ScreenRect,
    pub source: FrameSourceMetadata,
    pub content_epoch: u64,
    pub pixels: Option<RgbImage>,
    /// Capture-source frames delivered since the previous frame (WGC drop count;
    /// 1 for one-shot sources). See [`crate::wgc_capture::CaptureStats`].
    pub frames_delivered: u32,
    /// How stale the pixels were when consumed (0 for one-shot sources).
    pub staging_age: Duration,
}

/// Which backend produced a frame. `as_str` is the stable identifier written
/// into report JSON (`source.kind`), so the values are part of the artifact
/// schema documented for downstream tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSourceKind {
    ImageFile,
    WindowScreenshot,
    WindowsGraphicsCapture,
}

impl FrameSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ImageFile => "image_file",
            Self::WindowScreenshot => "window_screenshot",
            Self::WindowsGraphicsCapture => "windows_graphics_capture",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSourceMetadata {
    pub kind: FrameSourceKind,
    pub label: Option<String>,
    pub app_name: Option<String>,
    pub window_id: Option<u32>,
    pub pid: Option<u32>,
}

impl FrameSourceMetadata {
    pub fn image_file(path: impl Into<String>) -> Self {
        Self {
            kind: FrameSourceKind::ImageFile,
            label: Some(path.into()),
            app_name: None,
            window_id: None,
            pid: None,
        }
    }

    pub fn window(window: &xcap::Window) -> Self {
        Self {
            kind: FrameSourceKind::WindowScreenshot,
            label: window.title().ok(),
            app_name: window.app_name().ok(),
            window_id: window.id().ok(),
            pid: window.pid().ok(),
        }
    }
}

pub trait FrameSource {
    fn next_frame(&mut self) -> Result<Frame>;
}

/// Which window-capture path to use. Doubles as the `--capture-backend` CLI
/// value (the short `wgc`/`screenshot` names are kept as the canonical spelling,
/// with the descriptive names accepted as aliases).
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum WindowCapturePreference {
    /// Probe Windows Graphics Capture, fall back to the xcap screenshot path.
    Auto,
    #[value(name = "wgc", alias = "windows-graphics-capture")]
    WindowsGraphicsCapture,
    #[value(name = "screenshot", alias = "window-screenshot")]
    WindowScreenshot,
}

#[derive(Debug)]
pub struct ImageFileCapture {
    next_id: u64,
    image: RgbImage,
    fingerprint: u64,
    source_label: String,
}

impl ImageFileCapture {
    pub fn new(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let image = image::open(path)
            .with_context(|| format!("failed to open image {}", path.display()))?
            .to_rgb8();
        let fingerprint = fingerprint_rgb(&image);
        Ok(Self {
            next_id: 0,
            image,
            fingerprint,
            source_label: path.display().to_string(),
        })
    }
}

impl FrameSource for ImageFileCapture {
    fn next_frame(&mut self) -> Result<Frame> {
        let id = self.next_id;
        self.next_id += 1;
        let size = Size::new(self.image.width(), self.image.height());
        Ok(Frame {
            id,
            captured_at: Instant::now(),
            size,
            screen_rect: ScreenRect::new(0, 0, size),
            source: FrameSourceMetadata::image_file(self.source_label.clone()),
            content_epoch: self.fingerprint,
            pixels: Some(self.image.clone()),
            frames_delivered: 1,
            staging_age: Duration::ZERO,
        })
    }
}

#[derive(Debug)]
pub struct WindowScreenshotCapture {
    next_id: u64,
    selector: WindowSelector,
}

impl WindowScreenshotCapture {
    pub fn new(title_substring: impl Into<String>) -> Self {
        Self::with_selector(WindowSelector::title(title_substring))
    }

    pub fn with_selector(selector: WindowSelector) -> Self {
        Self {
            next_id: 0,
            selector,
        }
    }
}

impl FrameSource for WindowScreenshotCapture {
    fn next_frame(&mut self) -> Result<Frame> {
        let window = find_window(&self.selector)?;
        if window.is_minimized()? {
            bail!("target window {:?} is minimized", window.title()?);
        }

        let image = DynamicImage::ImageRgba8(window.capture_image()?).to_rgb8();
        let fingerprint = fingerprint_rgb(&image);
        let id = self.next_id;
        self.next_id += 1;
        let size = Size::new(image.width(), image.height());
        let source = FrameSourceMetadata::window(&window);

        Ok(Frame {
            id,
            captured_at: Instant::now(),
            size,
            screen_rect: ScreenRect::new(
                window.x().unwrap_or_default(),
                window.y().unwrap_or_default(),
                size,
            ),
            source,
            content_epoch: fingerprint,
            pixels: Some(image),
            frames_delivered: 1,
            staging_age: Duration::ZERO,
        })
    }
}

#[derive(Debug, Clone)]
struct CachedWindow {
    id: u32,
    pid: u32,
    app_name: String,
    title: String,
}

/// Max wait for the *first* WGC frame (session startup can be slow); steady-state
/// waits are capped much lower inside the session itself.
const WGC_INITIAL_FRAME_TIMEOUT: Duration = Duration::from_millis(3000);

#[derive(Debug)]
pub struct WindowsGraphicsCapture {
    next_id: u64,
    selector: WindowSelector,
    timeout: Duration,
    session: Option<ActiveWgcSession>,
    cached: Option<CachedWindow>,
}

impl WindowsGraphicsCapture {
    pub fn with_selector(selector: WindowSelector) -> Self {
        Self {
            next_id: 0,
            selector,
            timeout: WGC_INITIAL_FRAME_TIMEOUT,
            session: None,
            cached: None,
        }
    }

    /// Resolve the target window. Once resolved, geometry is refreshed via a cheap
    /// Win32 probe by HWND rather than re-enumerating every top-level window each
    /// frame (which cost ~34 ms/frame). Falls back to a full xcap re-resolve if the
    /// cached window is gone.
    fn resolve_window_info(&mut self) -> Result<WindowInfo> {
        if let Some(cached) = self.cached.clone() {
            use crate::wgc_capture::WindowProbe;
            match crate::wgc_capture::live_window_geometry(cached.id) {
                WindowProbe::Live(geometry) => {
                    return Ok(WindowInfo {
                        id: cached.id,
                        pid: cached.pid,
                        app_name: cached.app_name,
                        title: cached.title,
                        x: geometry.x,
                        y: geometry.y,
                        width: geometry.width,
                        height: geometry.height,
                    });
                }
                WindowProbe::Minimized => {
                    bail!("target window {:?} is minimized", cached.title)
                }
                WindowProbe::Gone => {
                    // Window vanished (e.g. closed/recreated); drop caches and
                    // re-resolve the selector from scratch below.
                    self.cached = None;
                    self.session = None;
                }
            }
        }

        let window = find_window(&self.selector)?;
        if window.is_minimized()? {
            bail!("target window {:?} is minimized", window.title()?);
        }
        let info = WindowInfo::from_window(&window)?;
        self.cached = Some(CachedWindow {
            id: info.id,
            pid: info.pid,
            app_name: info.app_name.clone(),
            title: info.title.clone(),
        });
        Ok(info)
    }

    fn capture_with_session(&mut self) -> Result<Frame> {
        let info = self.resolve_window_info()?;
        let session = match &mut self.session {
            Some(session) if session.window_id == info.id => session,
            _ => {
                self.session = Some(ActiveWgcSession {
                    window_id: info.id,
                    session: crate::wgc_capture::WindowCaptureSession::new(
                        info.id,
                        info.width,
                        info.height,
                        self.timeout,
                    )?,
                });
                self.session.as_mut().expect("WGC session was just created")
            }
        };

        let (image, stats) = session.session.capture_next()?;
        Ok(frame_from_window_image(self.next_id, &info, image, stats))
    }
}

impl FrameSource for WindowsGraphicsCapture {
    fn next_frame(&mut self) -> Result<Frame> {
        let frame = self.capture_with_session()?;
        self.next_id += 1;
        Ok(frame)
    }
}

#[derive(Debug)]
struct ActiveWgcSession {
    window_id: u32,
    session: crate::wgc_capture::WindowCaptureSession,
}

#[derive(Debug)]
pub struct AutoWindowCapture {
    selector: WindowSelector,
    active: Option<ActiveWindowCapture>,
}

impl AutoWindowCapture {
    pub fn with_selector(selector: WindowSelector) -> Self {
        Self {
            selector,
            active: None,
        }
    }
}

/// Why `AutoWindowCapture` abandoned WGC and fell back to xcap screenshots.
enum FallbackReason {
    /// WGC returned a frame, but it was blank/near-blank (common for 3D games
    /// whose surface WGC can't read).
    BlankProbe(Size),
    /// The WGC probe itself errored.
    WgcError(anyhow::Error),
}

impl FrameSource for AutoWindowCapture {
    fn next_frame(&mut self) -> Result<Frame> {
        if let Some(active) = &mut self.active {
            return active.next_frame();
        }

        let mut wgc = WindowsGraphicsCapture::with_selector(self.selector.clone());
        match wgc.next_frame() {
            Ok(frame) if frame_has_signal(&frame) => {
                self.active = Some(ActiveWindowCapture::Wgc(wgc));
                Ok(frame)
            }
            Ok(frame) => self.fall_back_to_screenshot(FallbackReason::BlankProbe(frame.size)),
            Err(error) => self.fall_back_to_screenshot(FallbackReason::WgcError(error)),
        }
    }
}

impl AutoWindowCapture {
    fn fall_back_to_screenshot(&mut self, reason: FallbackReason) -> Result<Frame> {
        let described = self.selector.describe();
        match &reason {
            FallbackReason::BlankProbe(_) => tracing::warn!(
                "WGC produced a blank or near-blank probe for {described}; falling back to xcap screenshot"
            ),
            FallbackReason::WgcError(error) => tracing::warn!(
                "WGC probe failed for {described}; falling back to xcap screenshot: {error:#}"
            ),
        }
        let (fallback, frame) =
            probe_screenshot_fallback(&self.selector).with_context(|| match &reason {
                FallbackReason::BlankProbe(size) => format!(
                    "WGC probe for {described} produced an unusable {}x{} frame and xcap fallback also failed",
                    size.width, size.height
                ),
                FallbackReason::WgcError(error) => format!(
                    "WGC probe failed for {described} ({error:#}) and xcap fallback also failed"
                ),
            })?;
        self.active = Some(ActiveWindowCapture::Screenshot(fallback));
        Ok(frame)
    }
}

#[derive(Debug)]
enum ActiveWindowCapture {
    Wgc(WindowsGraphicsCapture),
    Screenshot(WindowScreenshotCapture),
}

impl FrameSource for ActiveWindowCapture {
    fn next_frame(&mut self) -> Result<Frame> {
        match self {
            Self::Wgc(source) => source.next_frame(),
            Self::Screenshot(source) => source.next_frame(),
        }
    }
}

pub fn window_frame_source(
    selector: WindowSelector,
    preference: WindowCapturePreference,
) -> Box<dyn FrameSource + Send> {
    match preference {
        WindowCapturePreference::Auto => Box::new(AutoWindowCapture::with_selector(selector)),
        WindowCapturePreference::WindowsGraphicsCapture => {
            Box::new(WindowsGraphicsCapture::with_selector(selector))
        }
        WindowCapturePreference::WindowScreenshot => {
            Box::new(WindowScreenshotCapture::with_selector(selector))
        }
    }
}

fn probe_screenshot_fallback(
    selector: &WindowSelector,
) -> Result<(WindowScreenshotCapture, Frame)> {
    let mut fallback = WindowScreenshotCapture::with_selector(selector.clone());
    let frame = fallback.next_frame()?;
    Ok((fallback, frame))
}

fn frame_from_window_image(
    id: u64,
    info: &WindowInfo,
    image: RgbImage,
    stats: crate::wgc_capture::CaptureStats,
) -> Frame {
    let fingerprint = fingerprint_rgb(&image);
    let size = Size::new(image.width(), image.height());
    Frame {
        id,
        captured_at: Instant::now(),
        size,
        screen_rect: ScreenRect::new(info.x, info.y, size),
        source: FrameSourceMetadata {
            kind: FrameSourceKind::WindowsGraphicsCapture,
            label: Some(info.title.clone()),
            app_name: Some(info.app_name.clone()),
            window_id: Some(info.id),
            pid: Some(info.pid),
        },
        content_epoch: fingerprint,
        pixels: Some(image),
        frames_delivered: stats.frames_delivered,
        staging_age: stats.staging_age,
    }
}

fn frame_has_signal(frame: &Frame) -> bool {
    frame.pixels.as_ref().is_some_and(image_has_signal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_source_kind_strings_are_the_documented_schema() {
        // These strings appear in report JSON (`source.kind`) and are part of
        // the documented artifact schema — pin them so a rename can't silently
        // change the on-disk format.
        assert_eq!(FrameSourceKind::ImageFile.as_str(), "image_file");
        assert_eq!(
            FrameSourceKind::WindowScreenshot.as_str(),
            "window_screenshot"
        );
        assert_eq!(
            FrameSourceKind::WindowsGraphicsCapture.as_str(),
            "windows_graphics_capture"
        );
    }

    #[test]
    fn rejects_blank_capture_probe() {
        let image = RgbImage::from_pixel(1920, 1080, image::Rgb([0, 0, 0]));
        assert!(!image_has_signal(&image));
    }

    #[test]
    fn accepts_varied_capture_probe() {
        let mut image = RgbImage::from_pixel(128, 128, image::Rgb([12, 12, 12]));
        for y in 32..96 {
            for x in 32..96 {
                image.put_pixel(x, y, image::Rgb([240, 240, 240]));
            }
        }
        assert!(image_has_signal(&image));
    }
}
