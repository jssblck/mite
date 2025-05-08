//! Uses Paddle via ONNX

use std::sync::LazyLock;

use color_eyre::{Result, eyre::Context};
use paddle_ocr_rs::{
    ocr_lite::OcrLite,
    ocr_result::{Point, TextBlock},
};

use crate::{Annotation, Image, Vertice};

static ENGINE: LazyLock<OcrLite> = LazyLock::new(|| {
    eprintln!("init engine");
    let cpus = num_cpus::get();
    let models = workspace_root::get_workspace_root().join("models");
    let mut engine = OcrLite::new();
    engine
        .init_models(
            models
                .join("ch_PP-OCRv4_det_server_infer.onnx")
                .to_string_lossy()
                .to_string()
                .as_str(),
            models
                .join("ch_ppocr_mobile_v2.0_cls_infer.onnx")
                .to_string_lossy()
                .to_string()
                .as_str(),
            models
                .join("japan_PP-OCRv4_rec_infer.onnx")
                .to_string_lossy()
                .to_string()
                .as_str(),
            models
                .join("japan_dict.txt")
                .to_string_lossy()
                .to_string()
                .as_str(),
            cpus,
        )
        .expect("initialize models");
    engine
});

/// Set up the OCR engine.
pub fn install() {
    let _ = ENGINE;
}

/// Extract text from the image.
pub fn request(image: &Image) -> Result<Vec<Annotation>> {
    detect(image).map(|res| res.into_iter().map(Annotation::from).collect())
}

fn detect(Image(image): &Image) -> Result<Vec<TextBlock>> {
    // Adds margin around each detected text box to avoid clipping edge characters.
    // Increase if you notice partial characters in the output.
    let padding = 80;

    // Limits the largest image side for scaling. Reduces memory use and improves speed.
    // For screenshots, `1920` is safe. Drop to `1024–1280` for lower-end systems.
    let max_side_len = 1920;

    // Filters out weak text boxes from the detector.
    // Raise to reduce false positives. Lower if text is missed (e.g., light gray on white).
    let box_score_thresh = 0.5;

    // A secondary filter that affects post-processing of boxes.
    // Usually left alone unless tuning recall/precision balance.
    let box_thresh = 0.5;

    // Expands the size of each detected box outward to avoid tight cropping.
    // Increase if characters get cut off; decrease if adjacent lines are merging.
    let un_clip_ratio = 2.0;

    // Enables angle correction. Recommended for screenshots with rotated UI elements.
    let do_angle = true;

    // If true, aligns all detected boxes to the dominant angle.
    // Set to `true` for scans or uniform skew; leave `false` for mixed-orientation UIs.
    let most_angle = false;

    ENGINE
        .detect(
            &image,
            padding,
            max_side_len,
            box_score_thresh,
            box_thresh,
            un_clip_ratio,
            do_angle,
            most_angle,
        )
        .context("detect text in image")
        .map(|result| result.text_blocks)
}

impl From<TextBlock> for Annotation {
    fn from(block: TextBlock) -> Self {
        Self {
            text: block.text,
            bounds: block.box_points.into_iter().map(Vertice::from).collect(),
        }
    }
}

impl From<Point> for Vertice {
    fn from(p: Point) -> Self {
        Self {
            x: p.x as usize,
            y: p.y as usize,
        }
    }
}
