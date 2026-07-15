//! Desktop-app policy for grouping automatic eval captures by watched window.

use std::path::{Path, PathBuf};

/// Build the capture directory passed to `mite watch` for one selected window.
///
/// Keeping the normalization here gives every app launch the same filesystem-
/// safe layout while leaving the CLI's general-purpose output flag unchanged.
pub fn output_dir(root: &Path, window_title: &str, window_id: u32) -> PathBuf {
    let folder =
        normalize_window_title(window_title).unwrap_or_else(|| format!("window-{window_id}"));
    root.join(folder)
}

fn normalize_window_title(title: &str) -> Option<String> {
    const MAX_CHARS: usize = 80;

    let mut normalized = String::new();
    let mut separator_pending = false;

    for character in title.trim().chars() {
        if character.is_alphanumeric() {
            if separator_pending && !normalized.is_empty() && normalized.chars().count() < MAX_CHARS
            {
                normalized.push('-');
            }
            separator_pending = false;
            for lowercase in character.to_lowercase() {
                if normalized.chars().count() >= MAX_CHARS {
                    break;
                }
                normalized.push(lowercase);
            }
        } else if !normalized.is_empty() {
            separator_pending = true;
        }

        if normalized.chars().count() >= MAX_CHARS {
            break;
        }
    }

    let normalized = normalized.trim_end_matches('-');
    if normalized.is_empty() {
        return None;
    }

    let reserved = matches!(normalized, "con" | "prn" | "aux" | "nul")
        || normalized
            .strip_prefix("com")
            .or_else(|| normalized.strip_prefix("lpt"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });

    Some(if reserved {
        format!("window-{normalized}")
    } else {
        normalized.to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_window_titles_into_stable_folder_names() {
        assert_eq!(
            normalize_window_title("  Grace's Game: Deluxe Edition  ").as_deref(),
            Some("grace-s-game-deluxe-edition")
        );
        assert_eq!(
            normalize_window_title("日本語 Game [DX]").as_deref(),
            Some("日本語-game-dx")
        );
    }

    #[test]
    fn avoids_windows_reserved_names_and_empty_titles() {
        assert_eq!(normalize_window_title("CON").as_deref(), Some("window-con"));
        assert_eq!(
            normalize_window_title("COM²").as_deref(),
            Some("window-com²")
        );
        assert_eq!(
            output_dir(Path::new("C:\\eval"), "...", 42),
            Path::new("C:\\eval").join("window-42")
        );
    }

    #[test]
    fn bounds_folder_name_length() {
        let title = "a".repeat(200);
        assert_eq!(normalize_window_title(&title).unwrap().chars().count(), 80);
    }
}
