//! Powers OCR operations.
//!
//! Currently each module is self contained but if enough modules are added
//! they may be abstracted so they can be swapped in and out.

use std::path::Path;

use bon::Builder;
use color_eyre::Result;
use color_eyre::eyre::Context;
use image::RgbImage;

pub mod paddle;

/// Configure long living objects for use, like models or HTTP clients.
/// Call this once at program boot.
pub fn install() {
    paddle::install();
}

/// Image data for OCR.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Image(RgbImage);

impl Image {
    /// Synchronously load an image from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        image::open(path)
            .context("load image")
            .map(|i| i.into_rgb8())
            .map(Self)
    }
}

/// An annotation is an instance of discovered text in an image.
#[derive(Debug, Clone, Builder)]
#[non_exhaustive]
pub struct Annotation {
    /// The discovered text.
    #[builder(into)]
    pub text: String,

    /// The bounding polygon for the discovered text.
    #[builder(into)]
    pub bounds: Vec<Vertice>,
}

/// A point, or vertice, on the image.
#[derive(Debug, Clone, Copy, Builder)]
#[non_exhaustive]
pub struct Vertice {
    /// The `x` pixel coordinate for the vertice in the image.
    #[builder(into)]
    pub x: usize,

    /// The `y` pixel coordinate for the vertice in the image.
    #[builder(into)]
    pub y: usize,
}
