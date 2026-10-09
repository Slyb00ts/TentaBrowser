//! Key names as an agent writes them (`Enter`, `Control+a`, `ArrowDown`) and
//! the key event fields the page-side script takes.

use serde_json::{Value, json};

/// A parsed key chord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chord {
    pub key: String,
    pub code: String,
    pub key_code: u32,
    /// The text the key produces; empty for keys that produce none.
    pub text: String,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Chord {
    /// The init object of `__tentaCdp.key`.
    pub fn init(&self) -> Value {
        let mut init = json!({
            "key": self.key,
            "code": self.code,
            "keyCode": self.key_code,
            "text": self.text,
            "ctrlKey": self.ctrl,
            "shiftKey": self.shift,
            "altKey": self.alt,
            "metaKey": self.meta,
        });
        if self.ctrl && self.key.eq_ignore_ascii_case("a")
            || self.meta && self.key.eq_ignore_ascii_case("a")
        {
            init["commands"] = json!(["selectAll"]);
        }
        init
    }
}

const NAMED: &[(&str, &str, u32, &str)] = &[
    ("Enter", "Enter", 13, "\r"),
    ("Tab", "Tab", 9, ""),
    ("Escape", "Escape", 27, ""),
    ("Backspace", "Backspace", 8, ""),
    ("Delete", "Delete", 46, ""),
    ("ArrowUp", "ArrowUp", 38, ""),
    ("ArrowDown", "ArrowDown", 40, ""),
    ("ArrowLeft", "ArrowLeft", 37, ""),
    ("ArrowRight", "ArrowRight", 39, ""),
    ("Home", "Home", 36, ""),
    ("End", "End", 35, ""),
    ("PageUp", "PageUp", 33, ""),
    ("PageDown", "PageDown", 34, ""),
    ("Insert", "Insert", 45, ""),
];

fn alias(name: &str) -> &str {
    match name.to_ascii_lowercase().as_str() {
        "return" => "Enter",
        "esc" => "Escape",
        "del" => "Delete",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "pgup" | "pageup" => "PageUp",
        "pgdn" | "pagedown" => "PageDown",
        "enter" => "Enter",
        "tab" => "Tab",
        "escape" => "Escape",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "home" => "Home",
        "end" => "End",
        "insert" => "Insert",
        "space" => " ",
        _ => "",
    }
}

/// One key (not a chord): a named key, a function key or a single character.
pub fn single(name: &str) -> Result<Chord, String> {
    let aliased = alias(name);
    let name = if aliased.is_empty() { name } else { aliased };
    if let Some((key, code, key_code, text)) = NAMED.iter().find(|(key, ..)| *key == name) {
        return Ok(Chord {
            key: (*key).to_owned(),
            code: (*code).to_owned(),
            key_code: *key_code,
            text: (*text).to_owned(),
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
        });
    }
    if let Some(number) = name
        .strip_prefix('F')
        .and_then(|digits| digits.parse::<u32>().ok())
        && (1..=12).contains(&number)
    {
        return Ok(Chord {
            key: name.to_owned(),
            code: name.to_owned(),
            key_code: 111 + number,
            text: String::new(),
            ctrl: false,
            shift: false,
            alt: false,
            meta: false,
        });
    }
    let mut chars = name.chars();
    let (Some(character), None) = (chars.next(), chars.next()) else {
        return Err(format!(
            "unknown key `{name}`; use a character, a name such as Enter or ArrowDown, or a chord such as Control+a"
        ));
    };
    Ok(character_chord(character))
}

/// The key event fields for typing `character`.
pub fn character_chord(character: char) -> Chord {
    let (code, key_code) = match character {
        'a'..='z' => (
            format!("Key{}", character.to_ascii_uppercase()),
            u32::from(character.to_ascii_uppercase()),
        ),
        'A'..='Z' => (format!("Key{character}"), u32::from(character)),
        '0'..='9' => (format!("Digit{character}"), u32::from(character)),
        ' ' => ("Space".to_owned(), 32),
        _ => (String::new(), 0),
    };
    let text = if character == '\n' {
        "\r".to_owned()
    } else {
        character.to_string()
    };
    Chord {
        key: if character == '\n' {
            "Enter".to_owned()
        } else {
            character.to_string()
        },
        code: if character == '\n' {
            "Enter".to_owned()
        } else {
            code
        },
        key_code: if character == '\n' { 13 } else { key_code },
        text,
        ctrl: false,
        shift: character.is_ascii_uppercase(),
        alt: false,
        meta: false,
    }
}

/// Parses `Control+Shift+a`, `Enter`, `ArrowDown`, `+` (the plus key).
pub fn parse(spec: &str) -> Result<Chord, String> {
    if spec.is_empty() {
        return Err("the key is empty".to_owned());
    }
    if spec == "+" {
        return single("+");
    }
    // A lone plus is the plus key; a trailing plus after a modifier is too.
    let (modifiers, key) = match spec.rsplit_once('+') {
        Some((head, "")) if !head.is_empty() => (head.trim_end_matches('+'), "+"),
        Some((head, tail)) => (head, tail),
        None => ("", spec),
    };
    let mut chord = single(key)?;
    for modifier in modifiers.split('+').filter(|part| !part.is_empty()) {
        match modifier.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" | "option" => chord.alt = true,
            "meta" | "command" | "cmd" => chord.meta = true,
            other => return Err(format!("unknown modifier `{other}` in `{spec}`")),
        }
    }
    // Shortcuts produce commands, not text.
    if chord.ctrl || chord.alt || chord.meta {
        chord.text.clear();
    }
    Ok(chord)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_aliases_resolve() {
        for (spec, key, code) in [
            ("Enter", "Enter", "Enter"),
            ("esc", "Escape", "Escape"),
            ("down", "ArrowDown", "ArrowDown"),
        ] {
            let chord = parse(spec).unwrap();
            assert_eq!((chord.key.as_str(), chord.code.as_str()), (key, code));
        }
    }

    #[test]
    fn characters_carry_text_and_codes() {
        let chord = parse("a").unwrap();
        assert_eq!(
            (chord.code.as_str(), chord.key_code, chord.text.as_str()),
            ("KeyA", 65, "a")
        );
        let upper = parse("A").unwrap();
        assert!(upper.shift);
        assert_eq!(parse("7").unwrap().code, "Digit7");
    }

    #[test]
    fn chords_set_modifiers_and_drop_text() {
        let chord = parse("Control+Shift+a").unwrap();
        assert!(chord.ctrl && chord.shift && !chord.alt);
        assert!(chord.text.is_empty());
        assert_eq!(chord.init()["commands"][0], "selectAll");
        assert_eq!(parse("Control++").unwrap().key, "+");
        assert_eq!(parse("+").unwrap().key, "+");
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(parse("").is_err());
        assert!(parse("Hyper+a").is_err());
        assert!(parse("NotAKey").is_err());
    }
}
