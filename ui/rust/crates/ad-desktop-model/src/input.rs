/// Port of the existing USB HID usage to Linux evdev map.
pub fn remote_scan_code(usage: u32) -> Option<u16> {
    if usage >> 16 != 7 {
        return None;
    }
    match usage & 0xffff {
        4 => Some(30),
        5 => Some(48),
        6 => Some(46),
        7 => Some(32),
        8 => Some(18),
        9 => Some(33),
        10 => Some(34),
        11 => Some(35),
        12 => Some(23),
        13 => Some(36),
        14 => Some(37),
        15 => Some(38),
        16 => Some(50),
        17 => Some(49),
        18 => Some(24),
        19 => Some(25),
        20 => Some(16),
        21 => Some(19),
        22 => Some(31),
        23 => Some(20),
        24 => Some(22),
        25 => Some(47),
        26 => Some(17),
        27 => Some(45),
        28 => Some(21),
        29 => Some(44),
        30 => Some(2),
        31 => Some(3),
        32 => Some(4),
        33 => Some(5),
        34 => Some(6),
        35 => Some(7),
        36 => Some(8),
        37 => Some(9),
        38 => Some(10),
        39 => Some(11),
        40 => Some(28),
        41 => Some(1),
        42 => Some(14),
        43 => Some(15),
        44 => Some(57),
        45 => Some(12),
        46 => Some(13),
        47 => Some(26),
        48 => Some(27),
        49 => Some(43),
        50 => Some(86),
        51 => Some(39),
        52 => Some(40),
        53 => Some(41),
        54 => Some(51),
        55 => Some(52),
        56 => Some(53),
        57 => Some(58),
        58 => Some(59),
        59 => Some(60),
        60 => Some(61),
        61 => Some(62),
        62 => Some(63),
        63 => Some(64),
        64 => Some(65),
        65 => Some(66),
        66 => Some(67),
        67 => Some(68),
        68 => Some(87),
        69 => Some(88),
        70 => Some(99),
        71 => Some(70),
        72 => Some(119),
        73 => Some(110),
        74 => Some(102),
        75 => Some(104),
        76 => Some(111),
        77 => Some(107),
        78 => Some(109),
        79 => Some(106),
        80 => Some(105),
        81 => Some(108),
        82 => Some(103),
        83 => Some(69),
        84 => Some(98),
        85 => Some(55),
        86 => Some(74),
        87 => Some(78),
        88 => Some(96),
        89 => Some(79),
        90 => Some(80),
        91 => Some(81),
        92 => Some(75),
        93 => Some(76),
        94 => Some(77),
        95 => Some(71),
        96 => Some(72),
        97 => Some(73),
        98 => Some(82),
        99 => Some(83),
        100 => Some(86),
        101 => Some(127),
        224 => Some(29),
        225 => Some(42),
        226 => Some(56),
        227 => Some(125),
        228 => Some(97),
        229 => Some(54),
        230 => Some(100),
        231 => Some(126),
        _ => None,
    }
}
pub fn remote_lock_modifiers(num_lock: bool, caps_lock: bool) -> u16 {
    0x8000 | if num_lock { 0x10 } else { 0 } | if caps_lock { 0x20 } else { 0 }
}
/// Legacy shell control/Alt bindings. Terminal protocol negotiation is separate.
pub fn shell_shortcut(character: char, control: bool, alt: bool, shift: bool) -> Option<Vec<u8>> {
    let key = character.to_ascii_lowercase();
    let data = if control {
        if key.is_ascii_lowercase() && !shift {
            Some(vec![(key as u8) - b'a' + 1])
        } else if key == '_' || key == '-' {
            Some(vec![0x1f])
        } else {
            None
        }
    } else if alt && key.is_ascii() && !key.is_ascii_control() {
        Some(vec![if shift {
            key.to_ascii_uppercase() as u8
        } else {
            key as u8
        }])
    } else {
        None
    };
    data.map(|mut bytes| {
        if alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn physical_modifiers_and_layout_independent_keys_match_baseline() {
        assert_eq!(remote_scan_code(0x70004), Some(30));
        assert_eq!(remote_scan_code(0x700e0), Some(29));
        assert_eq!(remote_scan_code(0x700e6), Some(100));
        assert_eq!(remote_scan_code(0xc00e9), None);
        assert_eq!(remote_lock_modifiers(true, true), 0x8030);
    }
    #[test]
    fn readline_shortcuts_are_not_consumed_as_application_actions() {
        assert_eq!(shell_shortcut('a', true, false, false), Some(vec![1]));
        assert_eq!(shell_shortcut('w', true, false, false), Some(vec![23]));
        assert_eq!(shell_shortcut('_', true, false, true), Some(vec![31]));
        assert_eq!(
            shell_shortcut('f', false, true, false),
            Some(vec![27, b'f'])
        );
        assert_eq!(shell_shortcut('a', true, false, true), None);
    }
}
