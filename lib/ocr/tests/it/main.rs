//! Tests for the package.

use std::{path::PathBuf, sync::LazyLock, time::Instant};

fn fixtures() -> PathBuf {
    static ROOT: LazyLock<PathBuf> = LazyLock::new(workspace_root::get_workspace_root);
    ROOT.join("fixtures").join("ocr")
}

#[test]
fn fixture_mite() {
    eprintln!("initializing...");
    ocr::install();
    let _ = color_eyre::install();

    eprintln!("running ocr...");
    let img = ocr::Image::open(fixtures().join("mite.png")).expect("load image");
    let res = ocr::paddle::request(&img).expect("ocr image");

    eprintln!("results:");
    for item in res.iter() {
        eprintln!("  {}", item.text);
    }

    assert!(
        res.iter().any(|anno| anno.text.contains("みて")),
        "must contain 'みて' in annotations: {:?}",
        res,
    );
}

#[test]
fn fixture_mite_timing() {
    eprintln!("initializing...");
    ocr::install();
    let _ = color_eyre::install();

    let img = ocr::Image::open(fixtures().join("mite.png")).expect("load image");
    for round in 1..=5 {
        let start = Instant::now();

        let res = ocr::paddle::request(&img).expect("ocr image");
        assert!(
            res.iter().any(|anno| anno.text.contains("みて")),
            "must contain 'みて' in annotations: {:?}",
            res,
        );

        let elapsed = start.elapsed();
        eprintln!("[round {round}] {elapsed:?}");
    }
}
