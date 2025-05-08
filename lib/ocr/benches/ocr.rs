//! Benchmark tests for the OCR package.

#![allow(missing_docs)]

use std::path::PathBuf;
use std::sync::LazyLock;

use criterion::{Criterion, criterion_group, criterion_main};

/// Returns the path to the fixtures directory for OCR tests.
fn fixtures() -> PathBuf {
    static ROOT: LazyLock<PathBuf> = LazyLock::new(workspace_root::get_workspace_root);
    ROOT.join("fixtures").join("ocr")
}

/// Benchmark OCR processing using the PaddleOCR engine on a test image.
fn paddle(c: &mut Criterion) {
    ocr::install();
    let _ = color_eyre::install();

    let img = ocr::Image::open(fixtures().join("mite.png")).expect("load image");
    c.bench_function("ocr::paddle::request", |b| {
        b.iter(|| {
            let res = ocr::paddle::request(&img).expect("ocr image");
            assert!(
                res.iter().any(|anno| anno.text.contains("みて")),
                "must contain 'みて' in annotations: {:?}",
                res,
            );
        })
    });
}

criterion_group!(benches, paddle);
criterion_main!(benches);
