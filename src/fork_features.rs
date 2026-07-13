//! Thin adapters for features carried by the z233 fork.
//!
//! Core runtime and input primitives stay in their neutral modules. Feature-specific
//! state machines live here so upstream mode, copy-mode, and navigation files only
//! expose narrow hooks.

pub(crate) mod easymotion;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ForkFeatureState {
    pub easymotion: Option<crate::app::state::EasyMotionState>,
}

fn copy_mode_command_char(key: crate::input::TerminalKey) -> Option<char> {
    use crossterm::event::{KeyCode, KeyModifiers};

    if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return None;
    }
    if let Some(ch) = key.shifted_codepoint.and_then(char::from_u32) {
        return Some(ch);
    }
    let KeyCode::Char(ch) = key.code else {
        return None;
    };
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        Some(shifted_ascii_char(ch).unwrap_or(ch))
    } else {
        Some(ch)
    }
}

fn shifted_ascii_char(ch: char) -> Option<char> {
    match ch {
        'a'..='z' => Some(ch.to_ascii_uppercase()),
        '1' => Some('!'),
        '2' => Some('@'),
        '3' => Some('#'),
        '4' => Some('$'),
        '5' => Some('%'),
        '6' => Some('^'),
        '7' => Some('&'),
        '8' => Some('*'),
        '9' => Some('('),
        '0' => Some(')'),
        '-' => Some('_'),
        '=' => Some('+'),
        '[' => Some('{'),
        ']' => Some('}'),
        '\\' => Some('|'),
        ';' => Some(':'),
        '\'' => Some('"'),
        ',' => Some('<'),
        '.' => Some('>'),
        '/' => Some('?'),
        '`' => Some('~'),
        _ => None,
    }
}
