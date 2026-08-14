use std::collections::HashSet;
use std::fmt::{Display, Formatter};

pub const MOD_ALT_VALUE: u32 = 0x0001;
pub const MOD_CONTROL_VALUE: u32 = 0x0002;
pub const MOD_SHIFT_VALUE: u32 = 0x0004;
pub const MOD_WIN_VALUE: u32 = 0x0008;
pub const MOD_NOREPEAT_VALUE: u32 = 0x4000;
pub const DEFAULT_HOTKEY: &str = "Ctrl+DoubleF8";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyKind {
    Chord { modifiers: u32, virtual_key: u32 },
    CtrlMultiTap { taps: u8, virtual_key: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotkeySpec {
    pub kind: HotkeyKind,
    pub display: String,
}

#[derive(Debug, PartialEq, Eq)]
pub struct HotkeyError(pub String);

impl Display for HotkeyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for HotkeyError {}

pub fn parse_hotkey(value: &str) -> Result<HotkeySpec, HotkeyError> {
    let compact = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    for (prefix, taps, action) in [("CTRL+DOUBLE", 2, "双击"), ("CTRL+TRIPLE", 3, "三击")] {
        if let Some(key) = compact.strip_prefix(prefix).filter(|key| !key.is_empty()) {
            let (virtual_key, key_display) = parse_main_key(key)?;
            return Ok(HotkeySpec {
                kind: HotkeyKind::CtrlMultiTap { taps, virtual_key },
                display: format!("Ctrl + {action} {key_display}"),
            });
        }
    }
    let tokens: Vec<String> = value
        .split('+')
        .map(|part| part.trim().to_ascii_uppercase())
        .filter(|part| !part.is_empty())
        .collect();
    if tokens.is_empty() {
        return Err(HotkeyError("热键不能为空".into()));
    }

    let mut seen = HashSet::new();
    let mut modifiers = MOD_NOREPEAT_VALUE;
    let mut main_key: Option<(u32, String)> = None;

    for token in tokens {
        if !seen.insert(token.clone()) {
            return Err(HotkeyError(format!("热键包含重复按键：{token}")));
        }
        match token.as_str() {
            "CTRL" | "CONTROL" => modifiers |= MOD_CONTROL_VALUE,
            "ALT" => modifiers |= MOD_ALT_VALUE,
            "SHIFT" => modifiers |= MOD_SHIFT_VALUE,
            "WIN" | "WINDOWS" => modifiers |= MOD_WIN_VALUE,
            _ => {
                if main_key.is_some() {
                    return Err(HotkeyError("热键只能包含一个主键".into()));
                }
                main_key = Some(parse_main_key(&token)?);
            }
        }
    }

    if modifiers == MOD_NOREPEAT_VALUE {
        return Err(HotkeyError("热键至少需要一个修饰键".into()));
    }
    let (virtual_key, main_display) = main_key.ok_or_else(|| HotkeyError("热键缺少主键".into()))?;

    let mut display = Vec::new();
    if modifiers & MOD_CONTROL_VALUE != 0 {
        display.push("Ctrl".to_string());
    }
    if modifiers & MOD_ALT_VALUE != 0 {
        display.push("Alt".to_string());
    }
    if modifiers & MOD_SHIFT_VALUE != 0 {
        display.push("Shift".to_string());
    }
    if modifiers & MOD_WIN_VALUE != 0 {
        display.push("Win".to_string());
    }
    display.push(main_display);

    Ok(HotkeySpec {
        kind: HotkeyKind::Chord {
            modifiers,
            virtual_key,
        },
        display: display.join("+"),
    })
}

fn parse_main_key(token: &str) -> Result<(u32, String), HotkeyError> {
    let bytes = token.as_bytes();
    if bytes.len() == 1 && (bytes[0].is_ascii_uppercase() || bytes[0].is_ascii_digit()) {
        return Ok((bytes[0] as u32, token.into()));
    }
    if let Some(number) = token
        .strip_prefix('F')
        .and_then(|part| part.parse::<u32>().ok())
    {
        if number == 12 {
            return Err(HotkeyError("F12 是系统调试器保留键".into()));
        }
        if (1..=24).contains(&number) {
            return Ok((0x70 + number - 1, format!("F{number}")));
        }
    }
    Err(HotkeyError(format!("不支持的主键：{token}")))
}

pub fn check_hotkey_conflict(a: &HotkeySpec, b: &HotkeySpec) -> Result<(), HotkeyError> {
    if a == b {
        return Err(HotkeyError("快捷键不能相同".into()));
    }
    let vk_a = match a.kind {
        HotkeyKind::Chord { virtual_key, .. } | HotkeyKind::CtrlMultiTap { virtual_key, .. } => {
            virtual_key
        }
    };
    let vk_b = match b.kind {
        HotkeyKind::Chord { virtual_key, .. } | HotkeyKind::CtrlMultiTap { virtual_key, .. } => {
            virtual_key
        }
    };
    if vk_a == vk_b {
        let is_multi_a = matches!(a.kind, HotkeyKind::CtrlMultiTap { .. });
        let is_multi_b = matches!(b.kind, HotkeyKind::CtrlMultiTap { .. });
        if is_multi_a && is_multi_b {
            if let (
                HotkeyKind::CtrlMultiTap { taps: taps_a, .. },
                HotkeyKind::CtrlMultiTap { taps: taps_b, .. },
            ) = (&a.kind, &b.kind)
            {
                if taps_a == taps_b {
                    return Err(HotkeyError("同一按键不能重复设置相同次数的多击手势".into()));
                }
                // Different taps (e.g. 2 vs 3) on the same key are allowed for tier dispatch!
                return Ok(());
            }
        }
        if is_multi_a || is_multi_b {
            let chord = if is_multi_a { &b.kind } else { &a.kind };
            if let HotkeyKind::Chord { modifiers, .. } = chord {
                if *modifiers & MOD_CONTROL_VALUE != 0 {
                    return Err(HotkeyError(
                        "Ctrl 组合键与 Ctrl 多击手势不能使用相同的按键".into(),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn check_actions_hotkeys_conflict(actions: &[(&str, &HotkeySpec)]) -> Result<(), HotkeyError> {
    for i in 0..actions.len() {
        for j in (i + 1)..actions.len() {
            let (name_a, spec_a) = actions[i];
            let (name_b, spec_b) = actions[j];
            if let Err(e) = check_hotkey_conflict(spec_a, spec_b) {
                return Err(HotkeyError(format!(
                    "动作「{}」与「{}」的快捷键冲突：{}",
                    name_a, name_b, e.0
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_hotkeys() {
        let parsed = parse_hotkey(" shift + ctrl + o ").unwrap();
        assert_eq!(parsed.display, "Ctrl+Shift+O");
        assert_eq!(
            parsed.kind,
            HotkeyKind::Chord {
                modifiers: MOD_NOREPEAT_VALUE | MOD_CONTROL_VALUE | MOD_SHIFT_VALUE,
                virtual_key: b'O' as u32,
            }
        );
    }

    #[test]
    fn parses_legacy_ctrl_multi_tap_a_gestures() {
        assert_eq!(
            parse_hotkey(" Ctrl + TripleA ").unwrap(),
            HotkeySpec {
                kind: HotkeyKind::CtrlMultiTap {
                    taps: 3,
                    virtual_key: b'A' as u32,
                },
                display: "Ctrl + 三击 A".into(),
            }
        );
        assert_eq!(
            parse_hotkey("ctrl+doublea").unwrap().kind,
            HotkeyKind::CtrlMultiTap {
                taps: 2,
                virtual_key: b'A' as u32,
            }
        );
    }

    #[test]
    fn parses_ctrl_double_f8_as_a_multi_tap_gesture() {
        assert_eq!(
            parse_hotkey("Ctrl+DoubleF8").unwrap(),
            HotkeySpec {
                kind: HotkeyKind::CtrlMultiTap {
                    taps: 2,
                    virtual_key: 0x77,
                },
                display: "Ctrl + 双击 F8".into(),
            }
        );
    }

    #[test]
    fn supports_function_and_digit_keys() {
        assert_eq!(
            parse_hotkey("Alt+F24").unwrap().kind,
            HotkeyKind::Chord {
                modifiers: MOD_NOREPEAT_VALUE | MOD_ALT_VALUE,
                virtual_key: 0x87,
            }
        );
        assert_eq!(
            parse_hotkey("Win+1").unwrap().kind,
            HotkeyKind::Chord {
                modifiers: MOD_NOREPEAT_VALUE | MOD_WIN_VALUE,
                virtual_key: b'1' as u32,
            }
        );
    }

    #[test]
    fn rejects_invalid_combinations() {
        for value in [
            "O",
            "Ctrl",
            "Ctrl+O+P",
            "Ctrl+Ctrl+O",
            "Ctrl+F12",
            "Ctrl+Space",
            "Alt+TripleA",
        ] {
            assert!(parse_hotkey(value).is_err(), "{value} should be invalid");
        }
    }

    #[test]
    fn rejects_conflicting_hotkeys() {
        let double_f8 = parse_hotkey("Ctrl+DoubleF8").unwrap();
        let triple_f8 = parse_hotkey("Ctrl+TripleF8").unwrap();
        let ctrl_f8 = parse_hotkey("Ctrl+F8").unwrap();
        let double_f9 = parse_hotkey("Ctrl+DoubleF9").unwrap();
        let alt_f8 = parse_hotkey("Alt+F8").unwrap();

        assert!(check_hotkey_conflict(&double_f8, &double_f8).is_err());
        // Double and triple taps on the same key are allowed for tiered gestures!
        assert!(check_hotkey_conflict(&double_f8, &triple_f8).is_ok());
        assert!(check_hotkey_conflict(&double_f8, &ctrl_f8).is_err());
        assert!(check_hotkey_conflict(&double_f8, &double_f9).is_ok());
        assert!(check_hotkey_conflict(&double_f8, &alt_f8).is_ok());

        let actions = vec![
            ("提示词优化", &double_f8),
            ("智能翻译", &double_f9),
            ("代码重构", &triple_f8),
        ];
        assert!(check_actions_hotkeys_conflict(&actions).is_ok());

        let conflicting_actions = vec![("提示词优化", &double_f8), ("智能翻译", &ctrl_f8)];
        assert!(check_actions_hotkeys_conflict(&conflicting_actions).is_err());
    }
}
