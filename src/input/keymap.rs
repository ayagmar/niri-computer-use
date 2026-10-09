//! Resolves input against the compositor's unchanged XKB map, without I/O.

use xkbcommon::xkb;

use super::keyboard::{Typing, parse_combo};
use crate::error::{CallError, ErrorName, ToolError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Evdev(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Key {
    pub(crate) code: Evdev,
    pub(crate) modifiers: u32,
    pub(crate) group: u32,
    pub(crate) character: bool,
}

pub(super) fn resolve(map: &str, group: u32, typing: &Typing) -> Result<Vec<Key>, CallError> {
    let keymap = compile(map)?;
    if group >= keymap.num_layouts() {
        return Err(refused(
            "the active layout is absent from the compositor keymap",
        ));
    }
    match typing {
        Typing::Keys(combos) => combos
            .iter()
            .map(|combo| combo_key(&keymap, group, combo))
            .collect(),
        Typing::Text { text, submit } => {
            let mut keys = text
                .chars()
                .map(|c| {
                    let sym = match c {
                        '\n' => xkb::keysym_from_name("Return", xkb::KEYSYM_NO_FLAGS),
                        '\t' => xkb::keysym_from_name("Tab", xkb::KEYSYM_NO_FLAGS),
                        _ => xkb::utf32_to_keysym(u32::from(c)),
                    };
                    find(&keymap, group, sym).map(|mut key| {
                        key.character = true;
                        key
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if *submit {
                keys.push(combo_key(&keymap, group, "Return")?);
            }
            Ok(keys)
        }
    }
}

fn combo_key(map: &xkb::Keymap, group: u32, combo: &str) -> Result<Key, CallError> {
    let (modifiers, keysym_name) = parse_combo(combo).map_err(CallError::InvalidArguments)?;
    let mut key = find(
        map,
        group,
        xkb::keysym_from_name(keysym_name, xkb::KEYSYM_NO_FLAGS),
    )?;
    key.modifiers |= modifier_mask(map, &modifiers)?;
    Ok(key)
}

fn modifier_mask(map: &xkb::Keymap, modifiers: &[&str]) -> Result<u32, CallError> {
    let mut mask = 0;
    for &modifier in modifiers {
        let name = match modifier {
            "shift" => xkb::MOD_NAME_SHIFT,
            "ctrl" => xkb::MOD_NAME_CTRL,
            "alt" => xkb::MOD_NAME_ALT,
            "altgr" => xkb::MOD_NAME_ISO_LEVEL3_SHIFT,
            "logo" => xkb::MOD_NAME_LOGO,
            _ => return Err(refused("unsupported native modifier")),
        };
        let bit = 1_u32
            .checked_shl(map.mod_get_index(name))
            .ok_or_else(|| refused("modifier absent from compositor keymap"))?;
        mask |= bit;
    }
    Ok(mask)
}

pub(super) fn held_mask(map: &str, modifiers: &[&str]) -> Result<u32, CallError> {
    modifier_mask(&compile(map)?, modifiers)
}

fn compile(map: &str) -> Result<xkb::Keymap, CallError> {
    let context =
        xkb::Context::new(xkb::CONTEXT_NO_DEFAULT_INCLUDES | xkb::CONTEXT_NO_ENVIRONMENT_NAMES);
    xkb::Keymap::new_from_string(
        &context,
        map.to_owned(),
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .ok_or_else(|| refused("the compositor supplied an invalid XKB keymap"))
}

fn find(map: &xkb::Keymap, group: u32, sym: xkb::Keysym) -> Result<Key, CallError> {
    if sym.raw() == 0 {
        return Err(refused("unknown keysym; nothing was typed"));
    }
    let mut candidates = Vec::new();
    for raw in map.min_keycode().raw().max(8)..=map.max_keycode().raw() {
        let code = xkb::Keycode::new(raw);
        for level in 0..map.num_levels_for_key(code, group) {
            if map.key_get_syms_by_level(code, group, level) != [sym] {
                continue;
            }
            let mut masks = [0_u32; 32];
            let count = map.key_get_mods_for_level(code, group, level, &mut masks);
            candidates.extend(masks.iter().take(count).filter_map(|&modifiers| {
                let mut state = xkb::State::new(map);
                state.update_mask(modifiers, 0, 0, 0, 0, group);
                (state.key_get_one_sym(code) == sym).then_some(Key {
                    code: Evdev(raw - 8),
                    modifiers,
                    group,
                    character: false,
                })
            }));
        }
    }
    candidates.into_iter().min_by_key(|key| (key.modifiers.count_ones(), key.code.0))
        .ok_or_else(|| refused("a requested symbol is absent from the active compositor keymap; nothing was typed; select wtype explicitly for arbitrary Unicode"))
}

fn refused(detail: &str) -> CallError {
    ToolError::new(ErrorName::Refused, detail).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> String {
        include_str!("../../tests/fixtures/us-de.xkb").to_owned()
    }

    #[test]
    fn text_uses_the_active_layout_and_its_levels() {
        let map = map();
        let typing = Typing::Text {
            text: "yZ".into(),
            submit: false,
        };
        let us = resolve(&map, 0, &typing).unwrap();
        let de = resolve(&map, 1, &typing).unwrap();
        assert_ne!(us[0].code, de[0].code);
        assert_eq!(us[0].modifiers, 0);
        assert_ne!(us[1].modifiers, 0);
        let umlaut = Typing::Text {
            text: "ä€".into(),
            submit: true,
        };
        assert_eq!(resolve(&map, 1, &umlaut).unwrap().len(), 3);
        assert!(resolve(&map, 0, &umlaut).is_err());
    }

    #[test]
    fn missing_unicode_and_compose_dependent_text_refuse_the_whole_request() {
        let map = map();
        for text in ["ok→", "é", "a\u{301}"] {
            assert!(
                resolve(
                    &map,
                    0,
                    &Typing::Text {
                        text: text.into(),
                        submit: false
                    }
                )
                .is_err()
            );
        }
        assert!(resolve(&map, 2, &Typing::Keys(vec!["a".into()])).is_err());
    }
}
