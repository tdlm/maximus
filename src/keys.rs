use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

const MODS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SHIFT);

/// Normalize an event so `Char('P')` and `shift+p` compare equal, and so `ctrl+/` matches
/// the `ctrl+7` legacy terminals report for it.
fn normalize(code: KeyCode, mods: KeyModifiers) -> (KeyCode, KeyModifiers) {
    let mut mods = mods & MODS;
    let code = match code {
        KeyCode::Char('7') if mods.contains(KeyModifiers::CONTROL) => KeyCode::Char('/'),
        KeyCode::Char(c) if c.is_ascii_uppercase() => {
            mods |= KeyModifiers::SHIFT;
            KeyCode::Char(c.to_ascii_lowercase())
        }
        KeyCode::BackTab => {
            mods |= KeyModifiers::SHIFT;
            KeyCode::Tab
        }
        other => other,
    };
    (code, mods)
}

impl Binding {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        if s.is_empty() {
            return None;
        }
        let mut mods = KeyModifiers::NONE;
        let parts: Vec<&str> = if s.ends_with("++") {
            let mut v: Vec<&str> = s[..s.len() - 2].split('+').collect();
            v.push("+");
            v
        } else {
            s.split('+').collect()
        };
        let (key, modparts) = parts.split_last()?;
        for m in modparts {
            match *m {
                "ctrl" | "control" | "c" => mods |= KeyModifiers::CONTROL,
                "alt" | "opt" | "option" | "meta" | "m" => mods |= KeyModifiers::ALT,
                "shift" | "s" => mods |= KeyModifiers::SHIFT,
                _ => return None,
            }
        }
        let code = match *key {
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            k if k.starts_with('f') && k.len() > 1 && k[1..].parse::<u8>().is_ok() => {
                KeyCode::F(k[1..].parse().ok()?)
            }
            k if k.chars().count() == 1 => KeyCode::Char(k.chars().next()?),
            _ => return None,
        };
        let (code, mods) = normalize(code, mods);
        Some(Self { code, mods })
    }

    pub fn matches(&self, ev: &KeyEvent) -> bool {
        let (code, mods) = normalize(ev.code, ev.modifiers);
        code == self.code && mods == self.mods
    }

    pub fn from_event(ev: &KeyEvent) -> Self {
        let (code, mods) = normalize(ev.code, ev.modifiers);
        Self { code, mods }
    }

    /// Canonical config form, e.g. `ctrl+p`.
    pub fn to_config(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s += "ctrl+";
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s += "alt+";
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            s += "shift+";
        }
        s += &match self.code {
            KeyCode::Up => "up".into(),
            KeyCode::Down => "down".into(),
            KeyCode::Left => "left".into(),
            KeyCode::Right => "right".into(),
            KeyCode::Enter => "enter".into(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Backspace => "backspace".into(),
            KeyCode::Delete => "delete".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::PageUp => "pageup".into(),
            KeyCode::PageDown => "pagedown".into(),
            KeyCode::F(n) => format!("f{n}"),
            KeyCode::Char(c) => c.to_string(),
            _ => "?".into(),
        };
        s
    }

    /// Compact form for hints, e.g. `^p`, `⌥↓`.
    pub fn short(&self) -> String {
        let mut s = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            s += "^";
        }
        if self.mods.contains(KeyModifiers::ALT) {
            s += "⌥";
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            s += "⇧";
        }
        s += &match self.code {
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::Enter => "⏎".into(),
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Char(c) => c.to_string(),
            _ => self
                .to_config()
                .rsplit('+')
                .next()
                .unwrap_or("")
                .to_string(),
        };
        s
    }
}

/// A set of alternative bindings parsed from a space-separated string.
#[derive(Debug, Clone, Default)]
pub struct KeySet(pub Vec<Binding>);

impl KeySet {
    pub fn parse(s: &str) -> Self {
        Self(s.split_whitespace().filter_map(Binding::parse).collect())
    }
    pub fn matches(&self, ev: &KeyEvent) -> bool {
        self.0.iter().any(|b| b.matches(ev))
    }
    pub fn short(&self) -> String {
        self.0.first().map(|b| b.short()).unwrap_or_default()
    }
}

/// Encode a key event as the bytes a legacy xterm would send to a program.
pub fn encode(ev: &KeyEvent, app_cursor: bool) -> Vec<u8> {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    let alt = ev.modifiers.contains(KeyModifiers::ALT);
    let shift = ev.modifiers.contains(KeyModifiers::SHIFT);
    let modnum = 1 + shift as u8 + 2 * alt as u8 + 4 * ctrl as u8;
    let mut out = Vec::new();

    let csi_mod = |final_: char| -> Vec<u8> {
        if modnum > 1 {
            format!("\x1b[1;{modnum}{final_}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modnum > 1 {
            format!("\x1b[{n};{modnum}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };

    match ev.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            if ctrl {
                let b = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => (c as u8) & 0x1f,
                    '@' | ' ' | '2' => 0,
                    '[' | '3' => 0x1b,
                    '\\' | '4' => 0x1c,
                    ']' | '5' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' | '7' => 0x1f,
                    '?' | '8' => 0x7f,
                    _ => {
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        return out;
                    }
                };
                out.push(b);
            } else {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
        KeyCode::Enter => {
            // shift/alt+enter → meta-enter, which claude treats as a newline.
            if alt || shift {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab => {
            if shift {
                out.extend_from_slice(b"\x1b[Z");
            } else {
                out.push(b'\t');
            }
        }
        KeyCode::BackTab => out.extend_from_slice(b"\x1b[Z"),
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Esc => out.push(0x1b),
        KeyCode::Up => out = csi_mod('A'),
        KeyCode::Down => out = csi_mod('B'),
        KeyCode::Right => out = csi_mod('C'),
        KeyCode::Left => out = csi_mod('D'),
        KeyCode::Home => out = csi_mod('H'),
        KeyCode::End => out = csi_mod('F'),
        KeyCode::PageUp => out = tilde(5),
        KeyCode::PageDown => out = tilde(6),
        KeyCode::Delete => out = tilde(3),
        KeyCode::Insert => out = tilde(2),
        KeyCode::F(n) => {
            out = match n {
                1 => b"\x1bOP".to_vec(),
                2 => b"\x1bOQ".to_vec(),
                3 => b"\x1bOR".to_vec(),
                4 => b"\x1bOS".to_vec(),
                5 => tilde(15),
                6 => tilde(17),
                7 => tilde(18),
                8 => tilde(19),
                9 => tilde(20),
                10 => tilde(21),
                11 => tilde(23),
                12 => tilde(24),
                _ => vec![],
            }
        }
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_match() {
        let b = Binding::parse("ctrl+p").unwrap();
        assert!(b.matches(&KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)));
        assert!(!b.matches(&KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE)));
        let b = Binding::parse("alt+down").unwrap();
        assert!(b.matches(&KeyEvent::new(KeyCode::Down, KeyModifiers::ALT)));
        assert_eq!(b.short(), "⌥↓");
        let b = Binding::parse("ctrl+,").unwrap();
        assert_eq!(b.to_config(), "ctrl+,");
        let b = Binding::parse("ctrl+/").unwrap();
        assert!(b.matches(&KeyEvent::new(KeyCode::Char('/'), KeyModifiers::CONTROL)));
        assert!(b.matches(&KeyEvent::new(KeyCode::Char('7'), KeyModifiers::CONTROL)));
        assert!(!b.matches(&KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE)));
    }

    #[test]
    fn encodes() {
        let e = |c, m| encode(&KeyEvent::new(c, m), false);
        assert_eq!(e(KeyCode::Char('k'), KeyModifiers::CONTROL), vec![0x0b]);
        assert_eq!(e(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb".to_vec());
        assert_eq!(e(KeyCode::Up, KeyModifiers::NONE), b"\x1b[A".to_vec());
        assert_eq!(e(KeyCode::Left, KeyModifiers::ALT), b"\x1b[1;3D".to_vec());
        assert_eq!(e(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b\r".to_vec());
    }
}
