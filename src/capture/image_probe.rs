use image::RgbImage;

/// Smallest probe dimension worth analyzing; below this a frame is treated as
/// unusable regardless of content.
const MIN_PROBE_DIMENSION: u32 = 16;
/// Roughly how many pixels to sample across the frame (the stride is derived to
/// hit about this many).
const SIGNAL_SAMPLE_TARGET: usize = 4096;
/// A frame has signal only if more than this fraction of samples differ from the
/// first pixel (1/100 = 1%).
const MIN_CHANGED_SAMPLE_FRACTION: usize = 100;
/// ...and the sampled luma must span more than this (0-255), to reject a flat
/// near-black/near-white surface.
const MIN_LUMA_SPREAD: u16 = 4;

/// Whether a captured frame looks like real content rather than a blank/flat
/// surface (the common WGC failure mode for 3D game windows).
pub(super) fn image_has_signal(image: &RgbImage) -> bool {
    if image.width() < MIN_PROBE_DIMENSION || image.height() < MIN_PROBE_DIMENSION {
        return false;
    }

    let bytes = image.as_raw();
    let pixel_count = bytes.len() / 3;
    let stride = (pixel_count / SIGNAL_SAMPLE_TARGET).max(1);
    let mut samples = 0usize;
    let mut min_luma = u16::MAX;
    let mut max_luma = 0u16;
    let mut changed_pixels = 0usize;
    let first = &bytes[0..3];

    for pixel_index in (0..pixel_count).step_by(stride) {
        let offset = pixel_index * 3;
        let pixel = &bytes[offset..offset + 3];
        let luma = (u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2])) / 3;
        min_luma = min_luma.min(luma);
        max_luma = max_luma.max(luma);
        if pixel != first {
            changed_pixels += 1;
        }
        samples += 1;
    }

    changed_pixels > samples / MIN_CHANGED_SAMPLE_FRACTION
        && max_luma.saturating_sub(min_luma) > MIN_LUMA_SPREAD
}

/// FNV-1a hash over a strided sample of the image bytes, mixed with the
/// dimensions. A cheap content token: equal values mean almost certainly the
/// same frame, used to skip recognition on unchanged regions.
pub(super) fn fingerprint_rgb(image: &RgbImage) -> u64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    /// Sample every Nth byte (a prime, to avoid aligning with the 3-byte pixel
    /// stride and only ever hashing one channel).
    const SAMPLE_STRIDE: usize = 97;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in image.as_raw().iter().step_by(SAMPLE_STRIDE) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash ^ u64::from(image.width()) ^ (u64::from(image.height()) << 32)
}
