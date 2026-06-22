use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const APP_STORAGE_DIR: &str = "mite";
const IMAGE_EXTENSIONS: &[&str] = &["bmp", "gif", "jpeg", "jpg", "png", "webp"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCleanupReport {
    pub root: PathBuf,
    pub images: Vec<PathBuf>,
    pub dry_run: bool,
}

impl ImageCleanupReport {
    pub fn image_count(&self) -> usize {
        self.images.len()
    }
}

pub fn default_app_storage_root() -> Result<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
    Ok(PathBuf::from(local_app_data).join(APP_STORAGE_DIR))
}

pub fn clean_app_images(root: &Path, dry_run: bool) -> Result<ImageCleanupReport> {
    let display_root = root.to_path_buf();
    if !root.exists() {
        return Ok(ImageCleanupReport {
            root: display_root,
            images: Vec::new(),
            dry_run,
        });
    }
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }

    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", root.display()))?;
    let mut images = Vec::new();
    collect_images(&canonical_root, &canonical_root, &mut images)?;
    images.sort();

    if !dry_run {
        for image in &images {
            fs::remove_file(image)
                .with_context(|| format!("failed to delete {}", image.display()))?;
        }
    }

    Ok(ImageCleanupReport {
        root: display_root,
        images,
        dry_run,
    })
}

fn collect_images(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("failed to inspect {}", path.display()))?;

        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_images(root, &path, out)?;
            continue;
        }
        if !file_type.is_file() || !is_image_path(&path) {
            continue;
        }

        let canonical = path
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", path.display()))?;
        if !canonical.starts_with(root) {
            bail!(
                "refusing to clean image outside app storage: {}",
                canonical.display()
            );
        }
        out.push(canonical);
    }
    Ok(())
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_reports_images_without_deleting_them() {
        let temp = tempfile::tempdir().unwrap();
        let png = temp.path().join("capture.png");
        let text = temp.path().join("capture.json");
        fs::write(&png, b"not really a png").unwrap();
        fs::write(&text, b"{}").unwrap();

        let report = clean_app_images(temp.path(), true).unwrap();

        assert_eq!(report.image_count(), 1);
        assert!(png.exists());
        assert!(text.exists());
        assert!(report.dry_run);
    }

    #[test]
    fn deletes_images_recursively_and_keeps_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("eval-captures").join("capture-1");
        fs::create_dir_all(&nested).unwrap();
        let png = nested.join("underlying.PNG");
        let jpg = nested.join("frame.jpg");
        let json = nested.join("capture.json");
        fs::write(&png, b"png").unwrap();
        fs::write(&jpg, b"jpg").unwrap();
        fs::write(&json, b"{}").unwrap();

        let report = clean_app_images(temp.path(), false).unwrap();

        assert_eq!(report.image_count(), 2);
        assert!(!png.exists());
        assert!(!jpg.exists());
        assert!(json.exists());
    }

    #[test]
    fn missing_storage_root_is_empty_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("mite");

        let report = clean_app_images(&missing, false).unwrap();

        assert_eq!(report.image_count(), 0);
        assert_eq!(report.root, missing);
    }
}
