use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, RegisterHotKey,
    UnregisterHotKey,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyCombo {
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: HotkeyKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HotkeyKey {
    vk: u16,
    label: String,
}

#[derive(Debug)]
pub struct GlobalHotkey {
    id: i32,
}

impl HotkeyCombo {
    pub fn modifiers(&self) -> HOT_KEY_MODIFIERS {
        let mut modifiers = HOT_KEY_MODIFIERS(0);
        if self.ctrl {
            modifiers |= MOD_CONTROL;
        }
        if self.alt {
            modifiers |= MOD_ALT;
        }
        if self.shift {
            modifiers |= MOD_SHIFT;
        }
        if self.win {
            modifiers |= MOD_WIN;
        }
        modifiers | MOD_NOREPEAT
    }

    pub fn vk(&self) -> u32 {
        self.key.vk as u32
    }
}

impl GlobalHotkey {
    pub fn register(id: i32, combo: HotkeyCombo) -> Result<Self> {
        unsafe { RegisterHotKey(None, id, combo.modifiers(), combo.vk()) }
            .with_context(|| format!("failed to register global hotkey {combo}"))?;
        Ok(Self { id })
    }

    pub fn id(&self) -> i32 {
        self.id
    }
}

impl Drop for GlobalHotkey {
    fn drop(&mut self) {
        unsafe {
            let _ = UnregisterHotKey(None, self.id);
        }
    }
}

impl FromStr for HotkeyCombo {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut win = false;
        let mut key = None;

        for raw_part in value.split('+') {
            let part = raw_part.trim();
            if part.is_empty() {
                return Err("hotkey contains an empty segment".to_string());
            }
            let token = part.to_ascii_lowercase();
            match token.as_str() {
                "ctrl" | "control" => set_modifier(&mut ctrl, "Ctrl")?,
                "alt" => set_modifier(&mut alt, "Alt")?,
                "shift" => set_modifier(&mut shift, "Shift")?,
                "win" | "windows" | "super" => set_modifier(&mut win, "Win")?,
                _ => {
                    if key.is_some() {
                        return Err("hotkey must contain exactly one non-modifier key".to_string());
                    }
                    key = Some(parse_key(&token)?);
                }
            }
        }

        if !(ctrl || alt || shift || win) {
            return Err("hotkey must include at least one modifier".to_string());
        }
        let key = key.ok_or_else(|| "hotkey must include a non-modifier key".to_string())?;

        Ok(Self {
            ctrl,
            alt,
            shift,
            win,
            key,
        })
    }
}

impl fmt::Display for HotkeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.push(&self.key.label);
        write!(f, "{}", parts.join("+"))
    }
}

fn set_modifier(value: &mut bool, label: &str) -> std::result::Result<(), String> {
    if *value {
        return Err(format!("hotkey repeats the {label} modifier"));
    }
    *value = true;
    Ok(())
}

fn parse_key(token: &str) -> std::result::Result<HotkeyKey, String> {
    if token.len() == 1 {
        let byte = token.as_bytes()[0];
        if byte.is_ascii_alphanumeric() {
            let upper = byte.to_ascii_uppercase();
            return Ok(HotkeyKey {
                vk: upper as u16,
                label: char::from(upper).to_string(),
            });
        }
    }

    if let Some(number) = token.strip_prefix('f')
        && let Ok(number) = number.parse::<u16>()
        && (1..=24).contains(&number)
    {
        let label = format!("F{number}");
        return Ok(HotkeyKey {
            vk: 0x70 + number - 1,
            label,
        });
    }

    named_key(token).ok_or_else(|| {
        format!("unsupported hotkey key {token:?}; use A-Z, 0-9, F1-F24, or a common key name")
    })
}

fn named_key(token: &str) -> Option<HotkeyKey> {
    let (vk, label) = match token {
        "backspace" => (0x08, "Backspace"),
        "tab" => (0x09, "Tab"),
        "enter" | "return" => (0x0D, "Enter"),
        "esc" | "escape" => (0x1B, "Esc"),
        "space" => (0x20, "Space"),
        "pageup" | "pgup" => (0x21, "PageUp"),
        "pagedown" | "pgdn" => (0x22, "PageDown"),
        "end" => (0x23, "End"),
        "home" => (0x24, "Home"),
        "left" => (0x25, "Left"),
        "up" => (0x26, "Up"),
        "right" => (0x27, "Right"),
        "down" => (0x28, "Down"),
        "insert" | "ins" => (0x2D, "Insert"),
        "delete" | "del" => (0x2E, "Delete"),
        "printscreen" | "prtsc" | "prtscr" => (0x2C, "PrintScreen"),
        _ => return None,
    };
    Some(HotkeyKey {
        vk,
        label: label.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_canonicalizes_hotkey_combo() {
        let combo: HotkeyCombo = " ctrl + alt + f12 ".parse().unwrap();

        assert_eq!(combo.to_string(), "Ctrl+Alt+F12");
        assert_eq!(combo.vk(), 0x7B);
        assert_eq!(combo.modifiers(), MOD_CONTROL | MOD_ALT | MOD_NOREPEAT);
    }

    #[test]
    fn parses_named_keys_and_win_modifier() {
        let combo: HotkeyCombo = "win+shift+space".parse().unwrap();

        assert_eq!(combo.to_string(), "Shift+Win+Space");
        assert_eq!(combo.vk(), 0x20);
        assert_eq!(combo.modifiers(), MOD_SHIFT | MOD_WIN | MOD_NOREPEAT);
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_hotkeys() {
        assert!("F9".parse::<HotkeyCombo>().is_err());
        assert!("Ctrl+Alt".parse::<HotkeyCombo>().is_err());
        assert!("Ctrl+Ctrl+E".parse::<HotkeyCombo>().is_err());
        assert!("Ctrl+E+F".parse::<HotkeyCombo>().is_err());
        assert!("Ctrl+".parse::<HotkeyCombo>().is_err());
    }
}
