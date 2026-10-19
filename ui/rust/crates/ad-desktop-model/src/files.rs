//! File classification and round-trip encoding from file_action_policy.dart.
pub const MAX_EDITABLE_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    Utf16Le,
    Utf16Be,
}
pub struct TextFile {
    pub text: String,
    pub encoding: Encoding,
    pub bom: bool,
}
impl TextFile {
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_EDITABLE_BYTES {
            return None;
        }
        if bytes.starts_with(&[255, 254]) || bytes.starts_with(&[254, 255]) {
            if bytes.len() % 2 != 0 {
                return None;
            }
            let little = bytes[0] == 255;
            let units: Vec<_> = bytes[2..]
                .chunks_exact(2)
                .map(|pair| {
                    if little {
                        u16::from_le_bytes([pair[0], pair[1]])
                    } else {
                        u16::from_be_bytes([pair[0], pair[1]])
                    }
                })
                .collect();
            let text = String::from_utf16(&units).ok()?;
            if !valid_text(&text) {
                return None;
            }
            return Some(Self {
                text,
                encoding: if little {
                    Encoding::Utf16Le
                } else {
                    Encoding::Utf16Be
                },
                bom: true,
            });
        }
        let bom = bytes.starts_with(&[239, 187, 191]);
        let text = std::str::from_utf8(if bom { &bytes[3..] } else { bytes }).ok()?;
        if !valid_text(text) || text.starts_with("%PDF-") {
            return None;
        }
        Some(Self {
            text: text.into(),
            encoding: Encoding::Utf8,
            bom,
        })
    }
    pub fn encode(&self, text: &str) -> Result<Vec<u8>, &'static str> {
        if !valid_text(text) {
            return Err("Text contains binary control characters");
        }
        let mut bytes = Vec::new();
        match self.encoding {
            Encoding::Utf8 => {
                if self.bom {
                    bytes.extend_from_slice(&[239, 187, 191]);
                }
                bytes.extend_from_slice(text.as_bytes());
            }
            encoding => {
                let little = encoding == Encoding::Utf16Le;
                if self.bom {
                    bytes.extend_from_slice(if little { &[255, 254] } else { &[254, 255] });
                }
                for c in text.encode_utf16() {
                    bytes.extend_from_slice(&if little {
                        c.to_le_bytes()
                    } else {
                        c.to_be_bytes()
                    });
                }
            }
        }
        if bytes.len() > MAX_EDITABLE_BYTES {
            return Err("Text exceeds the 8 MiB editor limit");
        }
        Ok(bytes)
    }
}
fn valid_text(text: &str) -> bool {
    !text.chars().any(|c| {
        (c < ' ' && !matches!(c, '\t' | '\n' | '\x0c' | '\r')) || ('\u{7f}'..='\u{9f}').contains(&c)
    })
}
pub fn quote_terminal_path(path: &str) -> Result<String, &'static str> {
    if path.chars().any(|c| c < ' ' || c == '\x7f') {
        return Err("A terminal path cannot contain control characters");
    }
    Ok(format!("'{}'", path.replace('\'', "'\\''")))
}
pub fn safe_local_name(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|c| {
            if c < ' ' || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    safe = safe.trim_end_matches(['.', ' ']).into();
    if safe.is_empty() {
        safe = "remote-file".into();
    }
    // Truncate before checking device names, so truncation cannot introduce one.
    let length = safe.chars().count();
    if length > 180 {
        safe = safe.chars().skip(length - 180).collect();
    }
    let stem = safe.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix)
                .is_some_and(|n| n.len() == 1 && matches!(n.as_bytes()[0], b'1'..=b'9'))
        });
    if reserved {
        safe.insert(0, '_');
    }
    safe
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn editing_preserves_bom_unicode_and_line_endings() {
        for encoding in [Encoding::Utf8, Encoding::Utf16Le, Encoding::Utf16Be] {
            let file = TextFile {
                text: String::new(),
                encoding,
                bom: true,
            };
            let bytes = file.encode("first\r\né🦀\n").unwrap();
            let decoded = TextFile::decode(&bytes).unwrap();
            assert_eq!(decoded.encoding, encoding);
            assert_eq!(decoded.text, "first\r\né🦀\n");
            assert_eq!(decoded.encode(&decoded.text).unwrap(), bytes);
        }
    }
    #[test]
    fn binary_and_malformed_unicode_do_not_open_as_text() {
        for bytes in [
            &b"a\0b"[..],
            &b"%PDF-1.7"[..],
            &[255, 254, 0, 216][..],
            &[254, 255, 0][..],
        ] {
            assert!(TextFile::decode(bytes).is_none());
        }
        assert!(TextFile::decode(&vec![b'x'; MAX_EDITABLE_BYTES + 1]).is_none());
    }
    #[test]
    fn filenames_and_terminal_paths_cannot_escape_their_intended_role() {
        assert_eq!(safe_local_name("../../CON.txt"), ".._.._CON.txt");
        assert_eq!(safe_local_name("CON.txt"), "_CON.txt");
        assert_eq!(safe_local_name(".."), "remote-file");
        assert_eq!(safe_local_name("a:b"), "a_b");
        assert_eq!(quote_terminal_path("a'b").unwrap(), "'a'\\''b'");
        assert!(quote_terminal_path("file\nrm -rf /tmp").is_err());
    }
}
