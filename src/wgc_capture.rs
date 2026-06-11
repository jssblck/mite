use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::RgbImage;
use rayon::prelude::*;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindow};
use windows::core::{IInspectable, Interface, Ref, factory};

use crate::capture::FrameProbe;

/// Steady-state max wait for a fresh WGC frame (after the first). A changing
/// scene delivers frames well within this; a static one returns the last frame
/// after this, capping capture latency instead of stalling.
const STEADY_FRAME_TIMEOUT: Duration = Duration::from_millis(150);

/// Number of WGC frame-pool buffers. WGC delivers frames at the compositor
/// refresh rate — far faster than we consume (OCR runs at ~15 fps) — so we keep
/// only the most recent frame and let older ones be overwritten. Two buffers let
/// one frame be copied out while the next is delivered without dropping the
/// latest.
const FRAME_POOL_BUFFERS: i32 = 2;

/// Window geometry in screen coordinates.
#[derive(Debug, Clone, Copy)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Outcome of a [`live_window_geometry`] probe — the three states a tracked
/// window can be in, so the caller never has to remember what `Ok(None)` or an
/// error "meant".
#[derive(Debug)]
pub enum WindowProbe {
    /// The window exists and is visible; here is its current geometry.
    Live(WindowGeometry),
    /// The window exists but is minimized (no usable geometry right now).
    Minimized,
    /// The window no longer exists, or its rect could not be read, so the caller
    /// should re-resolve the selector from scratch.
    Gone,
}

/// Cheap per-frame geometry probe for an already-resolved window, by HWND. Avoids
/// re-enumerating every top-level window (the xcap `Window::all()` path costs tens
/// of milliseconds per frame).
pub fn live_window_geometry(hwnd_id: u32) -> WindowProbe {
    let hwnd = HWND(hwnd_id as usize as *mut std::ffi::c_void);
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return WindowProbe::Gone;
        }
        if IsIconic(hwnd).as_bool() {
            return WindowProbe::Minimized;
        }
        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return WindowProbe::Gone;
        }
        WindowProbe::Live(WindowGeometry {
            x: rect.left,
            y: rect.top,
            width: (rect.right - rect.left).max(0) as u32,
            height: (rect.bottom - rect.top).max(0) as u32,
        })
    }
}

/// Per-capture health stats, surfaced to the watch HUD/metrics.
#[derive(Debug, Clone, Copy)]
pub struct CaptureStats {
    /// WGC frames delivered since the previous `capture_next`. WGC delivers at the
    /// compositor refresh rate, so on a moving scene this is the number of frames
    /// dropped in favour of the freshest one (≈0 means we're keeping up exactly;
    /// a large value is expected, but a *regression* toward per-frame work would
    /// show as the consumer falling behind). Zero means no new frame arrived (a
    /// static scene served from the fallback).
    pub frames_delivered: u32,
    /// How long the returned frame sat in the staging slot before we consumed it
    /// (now − the moment it was copied in). The buffering staleness our capture
    /// path controls; small means fresh, growth signals a stall.
    pub staging_age: Duration,
}

/// A CPU-readable copy target. The producer copies each arrived WGC frame into it
/// (overwriting the previous, so it always holds the most recent frame); the
/// consumer maps it to build the RGB image. Reused across frames and only
/// recreated if the capture dimensions change.
struct StagingTexture {
    resource: ID3D11Resource,
    width: u32,
    height: u32,
}

/// Shared between the WGC pool thread (producer) and the worker thread
/// (consumer). The D3D11 immediate context is only ever touched while holding
/// this lock, which serializes the producer's copy against the consumer's map.
struct CaptureSlot {
    staging: Option<StagingTexture>,
    /// The staging texture holds a frame the consumer has not taken yet.
    fresh: bool,
    /// When the frame currently in `staging` was copied in (for staleness).
    stamped: Instant,
    /// WGC frames copied into staging since the consumer last took one.
    delivered: u32,
    /// Last producer-side error, surfaced if no frame is ever delivered.
    error: Option<String>,
}

struct CaptureShared {
    slot: Mutex<CaptureSlot>,
    arrived: Condvar,
}

pub struct WindowCaptureSession {
    shared: Arc<CaptureShared>,
    device_pair: D3dDevicePair,
    frame_pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    timeout: Duration,
    last_image: Option<Arc<RgbImage>>,
    /// Stamp of the most recent frame actually consumed, so the staleness of a
    /// fallback (stale) frame keeps growing while no new frame arrives.
    last_consumed_stamp: Instant,
}

impl std::fmt::Debug for WindowCaptureSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowCaptureSession")
            .field("timeout", &self.timeout)
            .field("has_last_image", &self.last_image.is_some())
            .finish()
    }
}

impl WindowCaptureSession {
    pub fn new(hwnd_id: u32, width: u32, height: u32, timeout: Duration) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("cannot capture an empty window: {width}x{height}");
        }

        let hwnd = HWND(hwnd_id as usize as *mut std::ffi::c_void);
        let device_pair = D3dDevicePair::new()?;
        let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .context("failed to load GraphicsCaptureItem interop factory")?;
        let item = unsafe { interop.CreateForWindow::<GraphicsCaptureItem>(hwnd) }
            .context("failed to create WGC item for window")?;

        let item_size = item.Size().context("failed to read WGC item size")?;
        let capture_width = width.min(item_size.Width as u32).max(1);
        let capture_height = height.min(item_size.Height as u32).max(1);
        let direct3d_device = device_pair.direct3d_device()?;
        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct3d_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            FRAME_POOL_BUFFERS,
            item_size,
        )
        .context("failed to create WGC frame pool")?;

        let shared = Arc::new(CaptureShared {
            slot: Mutex::new(CaptureSlot {
                staging: None,
                fresh: false,
                stamped: Instant::now(),
                delivered: 0,
                error: None,
            }),
            arrived: Condvar::new(),
        });

        frame_pool
            .FrameArrived(
                &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new({
                    let device = device_pair.device.clone();
                    let context = device_pair.context.clone();
                    let shared = shared.clone();
                    move |pool: Ref<'_, Direct3D11CaptureFramePool>, _| {
                        let Some(pool) = pool.as_ref() else {
                            return Ok(());
                        };
                        // Copy the freshest frame into the shared staging texture,
                        // overwriting whatever was there. No Map / CPU conversion
                        // here — the consumer does that once, on demand.
                        on_frame_arrived(
                            pool,
                            &device,
                            &context,
                            &shared,
                            capture_width,
                            capture_height,
                        );
                        Ok(())
                    }
                }),
            )
            .context("failed to register WGC frame handler")?;

        let session = frame_pool
            .CreateCaptureSession(&item)
            .context("failed to create WGC session")?;
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session
            .StartCapture()
            .context("failed to start WGC capture")?;

        Ok(Self {
            shared,
            device_pair,
            frame_pool,
            session,
            timeout,
            last_image: None,
            last_consumed_stamp: Instant::now(),
        })
    }

    pub fn capture_next(&mut self) -> Result<(Arc<RgbImage>, CaptureStats)> {
        match self.capture_next_probed(None)? {
            ProbedCapture::Frame(image, stats) => Ok((image, stats)),
            ProbedCapture::Unchanged(_) => unreachable!("no probe given"),
        }
    }

    /// Capture, optionally proving the scene unchanged from the mapped staging
    /// buffer instead of materializing an RGB frame. With a probe, a matching
    /// fresh frame (or no delivered frame at all) costs a few hundred point
    /// samples instead of the ~80 MB convert + retain + fingerprint path.
    pub fn capture_next_probed(&mut self, probe: Option<&FrameProbe>) -> Result<ProbedCapture> {
        // Wait the full timeout for the *first* frame (session startup can be
        // slow), but only briefly once we have a frame to fall back on — a static
        // scene delivers nothing, so returning a slightly-stale frame fast keeps
        // capture latency low (temporal smoothing reuses the detection anyway).
        let wait = if self.last_image.is_some() {
            STEADY_FRAME_TIMEOUT.min(self.timeout)
        } else {
            self.timeout
        };

        let mut slot = self
            .shared
            .slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let deadline = Instant::now() + wait;
        while !slot.fresh && slot.error.is_none() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, result) = self
                .shared
                .arrived
                .wait_timeout(slot, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            slot = next;
            if result.timed_out() {
                break;
            }
        }
        if slot.fresh {
            let Some(staging) = slot.staging.as_ref() else {
                slot.fresh = false;
                slot.delivered = 0;
                bail!("fresh WGC frame had no staging texture");
            };
            let stamp = slot.stamped;
            let stats = CaptureStats {
                frames_delivered: slot.delivered,
                staging_age: stamp.elapsed(),
            };
            let outcome = consume_staging(&self.device_pair.context, staging, probe)?;
            slot.fresh = false;
            slot.delivered = 0;
            drop(slot);

            self.last_consumed_stamp = stamp;
            return Ok(match outcome {
                StagingOutcome::Unchanged => ProbedCapture::Unchanged(stats),
                StagingOutcome::Image(image) => {
                    let image = Arc::new(image);
                    self.last_image = Some(Arc::clone(&image));
                    ProbedCapture::Frame(image, stats)
                }
            });
        }

        let error = slot.error.take();
        drop(slot);
        if let Some(image) = &self.last_image {
            // No new frame arrived: the scene cannot have changed. With a probe
            // that is already an answer; without one, serve the retained frame
            // (shared, so this never copies), staleness still accruing from when
            // it was originally captured.
            let stats = CaptureStats {
                frames_delivered: 0,
                staging_age: self.last_consumed_stamp.elapsed(),
            };
            return Ok(if probe.is_some() {
                ProbedCapture::Unchanged(stats)
            } else {
                ProbedCapture::Frame(Arc::clone(image), stats)
            });
        }
        if let Some(error) = error {
            bail!("WGC capture failed before the first frame: {error}");
        }
        bail!(
            "timed out after {} ms waiting for initial WGC frame",
            wait.as_millis()
        );
    }
}

/// Outcome of a probing capture.
pub enum ProbedCapture {
    Frame(Arc<RgbImage>, CaptureStats),
    Unchanged(CaptureStats),
}

enum StagingOutcome {
    Image(RgbImage),
    Unchanged,
}

/// Map the staging texture once and either prove the scene unchanged via the
/// probe (sampling a few hundred BGRA points) or convert the full frame.
fn consume_staging(
    context: &ID3D11DeviceContext,
    staging: &StagingTexture,
    probe: Option<&FrameProbe>,
) -> Result<StagingOutcome> {
    unsafe {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        context
            .Map(&staging.resource, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .context("failed to map WGC staging texture")?;
        let width = staging.width as usize;
        let height = staging.height as usize;
        let row_pitch = mapped.RowPitch as usize;
        let source =
            std::slice::from_raw_parts(mapped.pData as *const u8, height.max(1) * row_pitch);

        if let Some(probe) = probe
            && probe.expected_size == crate::geometry::Size::new(staging.width, staging.height)
            && probe.mean_diff(&sample_bgra_probe(source, row_pitch, probe)) <= probe.max_mean_diff
        {
            context.Unmap(&staging.resource, 0);
            return Ok(StagingOutcome::Unchanged);
        }

        let rgb = bgra_to_rgb(source, width, height, row_pitch);
        context.Unmap(&staging.resource, 0);
        let image = RgbImage::from_raw(staging.width, staging.height, rgb)
            .context("failed to build RGB image from WGC frame")?;
        Ok(StagingOutcome::Image(image))
    }
}

/// Sample the probe's points from a row-padded BGRA buffer, producing the same
/// luma values [`FrameProbe::matches_rgb`] would compute on the converted
/// frame (the swizzle is applied per point).
fn sample_bgra_probe(source: &[u8], row_pitch: usize, probe: &FrameProbe) -> Vec<u8> {
    probe
        .points
        .iter()
        .map(|&(x, y)| {
            let offset = y as usize * row_pitch + x as usize * 4;
            let b = source[offset];
            let g = source[offset + 1];
            let r = source[offset + 2];
            FrameProbe::luma(r, g, b)
        })
        .collect()
}

impl Drop for WindowCaptureSession {
    fn drop(&mut self) {
        self.session.Close().ok();
        self.frame_pool.Close().ok();
    }
}

#[derive(Debug)]
struct D3dDevicePair {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
}

impl D3dDevicePair {
    fn new() -> Result<Self> {
        let mut device = None;
        let mut context = None;
        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .context("failed to create D3D11 device")?;
        }
        Ok(Self {
            device: device.context("D3D11CreateDevice returned no device")?,
            context: context.context("D3D11CreateDevice returned no immediate context")?,
        })
    }

    fn direct3d_device(&self) -> Result<IDirect3DDevice> {
        let dxgi = self
            .device
            .cast::<IDXGIDevice>()
            .context("failed to cast D3D11 device to DXGI device")?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
            .context("failed to create WinRT Direct3D device")?;
        inspectable
            .cast::<IDirect3DDevice>()
            .context("failed to cast WinRT device")
    }
}

/// Producer side: runs on the WGC pool thread for every delivered frame. Pulls
/// the frame and copies it (GPU-side) into the shared staging texture, then
/// signals the consumer. Errors are stored rather than propagated (there is no
/// caller to return them to) and surfaced by the consumer if no frame arrives.
fn on_frame_arrived(
    pool: &Direct3D11CaptureFramePool,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    shared: &CaptureShared,
    width: u32,
    height: u32,
) {
    let frame = match pool.TryGetNextFrame() {
        Ok(frame) => frame,
        Err(error) => {
            store_error(shared, format!("failed to get WGC frame: {error}"));
            return;
        }
    };

    if let Err(error) = copy_frame_into_staging(&frame, device, context, shared, width, height) {
        store_error(shared, format!("{error:#}"));
    }
    frame.Close().ok();
}

fn copy_frame_into_staging(
    frame: &Direct3D11CaptureFrame,
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    shared: &CaptureShared,
    width: u32,
    height: u32,
) -> Result<()> {
    let surface = frame
        .Surface()
        .context("failed to read WGC frame surface")?;
    let access = surface
        .cast::<IDirect3DDxgiInterfaceAccess>()
        .context("failed to access DXGI surface")?;
    let texture = unsafe { access.GetInterface::<ID3D11Texture2D>() }
        .context("failed to get D3D11 texture")?;

    let mut slot = shared
        .slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    copy_texture_into_staging(device, context, &mut slot.staging, &texture, width, height)?;
    slot.fresh = true;
    slot.stamped = Instant::now();
    slot.delivered = slot.delivered.saturating_add(1);
    slot.error = None;
    drop(slot);
    shared.arrived.notify_one();
    Ok(())
}

fn store_error(shared: &CaptureShared, message: String) {
    let mut slot = shared
        .slot
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    slot.error = Some(message);
    shared.arrived.notify_one();
}

fn ensure_staging<'a>(
    cache: &'a mut Option<StagingTexture>,
    device: &ID3D11Device,
    source_desc: &D3D11_TEXTURE2D_DESC,
    width: u32,
    height: u32,
) -> Result<&'a StagingTexture> {
    let reuse = cache
        .as_ref()
        .is_some_and(|staging| staging.width == width && staging.height == height);
    if !reuse {
        let mut staging_desc = *source_desc;
        staging_desc.Width = width;
        staging_desc.Height = height;
        staging_desc.BindFlags = 0;
        staging_desc.MiscFlags = 0;
        staging_desc.Usage = D3D11_USAGE_STAGING;
        staging_desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;

        let mut staging = None;
        unsafe { device.CreateTexture2D(&staging_desc, None, Some(&mut staging)) }
            .context("failed to create WGC staging texture")?;
        let texture = staging.context("CreateTexture2D returned no staging texture")?;
        let resource: ID3D11Resource = texture.cast()?;
        *cache = Some(StagingTexture {
            resource,
            width,
            height,
        });
    }
    cache
        .as_ref()
        .context("staging texture was not initialized")
}

/// GPU-side copy of the captured frame into the reused staging texture. Cheap
/// (no CPU readback); the expensive `Map` happens later, once, on the consumer.
fn copy_texture_into_staging(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    cache: &mut Option<StagingTexture>,
    texture: &ID3D11Texture2D,
    width: u32,
    height: u32,
) -> Result<()> {
    unsafe {
        let mut source_desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut source_desc);
        let width = width.min(source_desc.Width);
        let height = height.min(source_desc.Height);

        let staging = ensure_staging(cache, device, &source_desc, width, height)?;
        let source_box = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: width,
            bottom: height,
            back: 1,
        };
        let source_resource: ID3D11Resource = texture.cast()?;
        context.CopySubresourceRegion(
            &staging.resource,
            0,
            0,
            0,
            0,
            &source_resource,
            0,
            Some(&source_box),
        );
    }
    Ok(())
}

/// Convert a row-padded BGRA staging buffer (`row_pitch` ≥ `width*4`) into a tight
/// RGB buffer. Parallelized across output rows with rayon: at 4K this is ~8.3M
/// pixels of channel-swizzle per frame and was a meaningful slice of the capture
/// stage when run single-threaded. Rows are independent and write to disjoint
/// output chunks, so this scales cleanly across cores.
fn bgra_to_rgb(source: &[u8], width: usize, height: usize, row_pitch: usize) -> Vec<u8> {
    // ~25 MB at 4K, fully overwritten below; skip the zero-initialization that
    // `vec![0; n]` would pay (and its first-touch page faults) by writing into
    // spare capacity.
    let len = width * height * 3;
    let mut rgb: Vec<u8> = Vec::with_capacity(len);
    rgb.spare_capacity_mut()[..len]
        .par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(row, dst_row)| {
            let src_start = row * row_pitch;
            let src_row = &source[src_start..src_start + width * 4];
            for (src, dst) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(3)) {
                dst[0].write(src[2]);
                dst[1].write(src[1]);
                dst[2].write(src[0]);
            }
        });
    // SAFETY: the parallel loop above wrote every byte in `..len` (height rows
    // of exactly width*3 bytes each).
    unsafe { rgb.set_len(len) };
    rgb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_to_rgb_swizzles_and_drops_row_padding() {
        // 2×2 BGRA with a padded row pitch (10 bytes/row vs 8 used). Pixels are
        // distinct so a wrong channel order or a pitch bug would show.
        let width = 2;
        let height = 2;
        let row_pitch = 10;
        let mut source = vec![0u8; height * row_pitch];
        // Row 0: (B,G,R,A) = (1,2,3,255), (4,5,6,255)
        source[0..8].copy_from_slice(&[1, 2, 3, 255, 4, 5, 6, 255]);
        // Row 1: (7,8,9,255), (10,11,12,255)
        source[row_pitch..row_pitch + 8].copy_from_slice(&[7, 8, 9, 255, 10, 11, 12, 255]);

        let rgb = bgra_to_rgb(&source, width, height, row_pitch);
        // Expect R,G,B per pixel (B/R swapped from source), padding ignored.
        assert_eq!(rgb, vec![3, 2, 1, 6, 5, 4, 9, 8, 7, 12, 11, 10]);
    }

    #[test]
    fn bgra_probe_sampling_matches_rgb_probe_evaluation() -> Result<()> {
        // A probe evaluated on the raw BGRA staging buffer must produce the
        // same luma samples as evaluating on the converted RGB image, so the
        // capture-side fast path and the worker-side fallback agree.
        let width = 7usize;
        let height = 5usize;
        let row_pitch = width * 4 + 12;
        let mut source = vec![0u8; height * row_pitch];
        for y in 0..height {
            for x in 0..width {
                let offset = y * row_pitch + x * 4;
                source[offset] = (x * 40 + y) as u8; // B
                source[offset + 1] = (x * 11 + y * 31) as u8; // G
                source[offset + 2] = (x * 7 + y * 13) as u8; // R
                source[offset + 3] = 255;
            }
        }
        let rgb = bgra_to_rgb(&source, width, height, row_pitch);
        let image = RgbImage::from_raw(width as u32, height as u32, rgb)
            .context("RGB probe image buffer did not match dimensions")?;

        let points: Vec<(u32, u32)> = vec![(0, 0), (6, 4), (3, 2), (6, 0), (0, 4)];
        let expected: Vec<u8> = points
            .iter()
            .map(|&(x, y)| {
                let p = image.get_pixel(x, y);
                FrameProbe::luma(p[0], p[1], p[2])
            })
            .collect();
        let probe = FrameProbe {
            expected_size: crate::geometry::Size::new(width as u32, height as u32),
            points: points.clone(),
            luma: expected.clone(),
            max_mean_diff: 0,
        };

        assert_eq!(sample_bgra_probe(&source, row_pitch, &probe), expected);
        assert!(probe.matches_rgb(&image));
        Ok(())
    }
}
