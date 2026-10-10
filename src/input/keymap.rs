//! Resolves input against the compositor's XKB map, without I/O. Text symbols the active
//! layout lacks go on spare keys of an extended copy; every existing key stays the same.

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

/// What to type: the keys, and the map to upload for the call when the active layout
/// lacks some of the text's symbols.
#[derive(Debug)]
pub(super) struct Plan {
    pub(super) keys: Vec<Key>,
    /// The compositor's map with the missing symbols on spare keys, serialized by
    /// libxkbcommon as the compositor re-serializes it.
    pub(super) extended: Option<String>,
}

/// The highest keycode Xwayland clients can receive.
const MAX_X11_KEYCODE: u32 = 255;

pub(super) fn plan(map: &str, group: u32, typing: &Typing) -> Result<Plan, CallError> {
    let keymap = compile(map)?;
    if group >= keymap.num_layouts() {
        return Err(refused(
            "the active layout is absent from the compositor keymap",
        ));
    }
    let missing = missing(&keymap, group, typing)?;
    if missing.is_empty() {
        return Ok(Plan {
            keys: resolve(&keymap, group, typing)?,
            extended: None,
        });
    }
    let extended = extend(map, &keymap, &missing)?;
    Ok(Plan {
        keys: resolve(&compile(&extended)?, group, typing)?,
        extended: Some(extended),
    })
}

fn resolve(keymap: &xkb::Keymap, group: u32, typing: &Typing) -> Result<Vec<Key>, CallError> {
    match typing {
        Typing::Keys(combos) => combos
            .iter()
            .map(|combo| combo_key(keymap, group, combo))
            .collect(),
        Typing::Text { text, submit } => {
            let mut keys = text
                .chars()
                .map(|c| {
                    find(keymap, group, text_sym(c)).map(|mut key| {
                        key.character = true;
                        key
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if *submit {
                keys.push(combo_key(keymap, group, "Return")?);
            }
            Ok(keys)
        }
    }
}

fn text_sym(c: char) -> xkb::Keysym {
    match c {
        '\n' => xkb::keysym_from_name("Return", xkb::KEYSYM_NO_FLAGS),
        '\t' => xkb::keysym_from_name("Tab", xkb::KEYSYM_NO_FLAGS),
        _ => xkb::utf32_to_keysym(u32::from(c)),
    }
}

/// The text's distinct printable symbols the active layout lacks, in order of use.
fn missing(
    keymap: &xkb::Keymap,
    group: u32,
    typing: &Typing,
) -> Result<Vec<xkb::Keysym>, CallError> {
    let Typing::Text { text, .. } = typing else {
        return Ok(Vec::new());
    };
    let mut missing = Vec::new();
    for c in text.chars() {
        let sym = text_sym(c);
        if find(keymap, group, sym).is_ok() || missing.contains(&sym) {
            continue;
        }
        if sym.raw() == 0 || c.is_control() {
            return Err(refused(
                "a control character is absent from the active compositor keymap; nothing was typed",
            ));
        }
        missing.push(sym);
    }
    Ok(missing)
}

/// Puts each symbol on a named key that has no symbols, at most `MAX_X11_KEYCODE`, and
/// proves every other key, the layouts and the modifiers unchanged.
fn extend(map: &str, keymap: &xkb::Keymap, syms: &[xkb::Keysym]) -> Result<String, CallError> {
    let spare = spare_keys(keymap);
    if syms.len() > spare.len() {
        return Err(refused(&format!(
            "{} distinct symbols are absent from the active layout and only {} spare keys exist; nothing was typed; split the text or select wtype",
            syms.len(),
            spare.len()
        )));
    }
    let entries = spare
        .iter()
        .zip(syms)
        .map(|((_, name), &sym)| format!("key <{name}> {{ [ {} ] }};\n", xkb::keysym_get_name(sym)))
        .collect::<Vec<_>>()
        .concat();
    let end = symbols_end(map).ok_or_else(|| {
        refused("the compositor keymap has an unexpected layout; nothing was typed")
    })?;
    let (head, tail) = map.split_at(end);
    let extended = compile(&format!("{head}{entries}{tail}"))?;
    let used: Vec<xkb::Keycode> = spare
        .iter()
        .take(syms.len())
        .map(|(code, _)| *code)
        .collect();
    if !unchanged(keymap, &extended, &used) {
        return Err(refused(
            "extending the compositor keymap would change existing keys; nothing was typed",
        ));
    }
    Ok(extended.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
}

fn spare_keys(keymap: &xkb::Keymap) -> Vec<(xkb::Keycode, String)> {
    let first = keymap.min_keycode().raw().max(9);
    let last = keymap.max_keycode().raw().min(MAX_X11_KEYCODE);
    (first..=last)
        .map(xkb::Keycode::new)
        .filter(|&code| keymap.num_layouts_for_key(code) == 0)
        .filter_map(|code| Some((code, keymap.key_get_name(code)?.to_owned())))
        .collect()
}

/// Where the symbols section closes: the last `};` before the keymap's own.
fn symbols_end(map: &str) -> Option<usize> {
    let keymap_end = map.rfind("};")?;
    let end = map.get(..keymap_end)?.rfind("};")?;
    let symbols = map.find("xkb_symbols")?;
    let after = map.get(symbols..end)?;
    let other = ["xkb_keycodes", "xkb_types", "xkb_compat", "xkb_geometry"];
    (!other.iter().any(|section| after.contains(section))).then_some(end)
}

fn unchanged(original: &xkb::Keymap, extended: &xkb::Keymap, added: &[xkb::Keycode]) -> bool {
    let mods = |map: &xkb::Keymap| -> Vec<String> {
        (0..map.num_mods())
            .map(|index| map.mod_get_name(index).to_owned())
            .collect()
    };
    original.num_layouts() == extended.num_layouts()
        && mods(original) == mods(extended)
        && (original.min_keycode().raw()..=original.max_keycode().raw())
            .map(xkb::Keycode::new)
            .filter(|code| !added.contains(code))
            .all(|code| same_key(original, extended, code))
        && added.iter().all(|&code| inert(extended, code))
}

fn same_key(original: &xkb::Keymap, extended: &xkb::Keymap, code: xkb::Keycode) -> bool {
    let layouts = original.num_layouts_for_key(code);
    layouts == extended.num_layouts_for_key(code)
        && (0..layouts).all(|layout| {
            let levels = original.num_levels_for_key(code, layout);
            levels == extended.num_levels_for_key(code, layout)
                && (0..levels).all(|level| {
                    original.key_get_syms_by_level(code, layout, level)
                        == extended.key_get_syms_by_level(code, layout, level)
                        && level_mods(original, code, layout, level)
                            == level_mods(extended, code, layout, level)
                })
        })
}

fn level_mods(map: &xkb::Keymap, code: xkb::Keycode, layout: u32, level: u32) -> Vec<u32> {
    let mut masks = [0_u32; 32];
    let count = map.key_get_mods_for_level(code, layout, level, &mut masks);
    masks.iter().take(count).copied().collect()
}

/// An added key types its symbol and changes no modifier or layout.
fn inert(map: &xkb::Keymap, code: xkb::Keycode) -> bool {
    let mut state = xkb::State::new(map);
    state.update_key(code, xkb::KeyDirection::Down);
    map.num_layouts_for_key(code) == 1
        && state.serialize_mods(xkb::STATE_MODS_EFFECTIVE) == 0
        && state.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE) == 0
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

/// `map` as libxkbcommon serializes it once compiled, as niri re-serializes a map a device
/// uploads; `None` if it doesn't compile. Two spellings of one map give the same text.
pub(crate) fn serialized(map: &str) -> Option<String> {
    compile(map)
        .ok()
        .map(|keymap| keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1))
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
    candidates
        .into_iter()
        .min_by_key(|key| (key.modifiers.count_ones(), key.code.0))
        .ok_or_else(|| {
            refused(
                "a requested symbol is absent from the active compositor keymap; nothing was typed",
            )
        })
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
        let us = plan(&map, 0, &typing).unwrap().keys;
        let de = plan(&map, 1, &typing).unwrap().keys;
        assert_ne!(us[0].code, de[0].code);
        assert_eq!(us[0].modifiers, 0);
        assert_ne!(us[1].modifiers, 0);
        let umlaut = Typing::Text {
            text: "ä€".into(),
            submit: true,
        };
        assert_eq!(plan(&map, 1, &umlaut).unwrap().keys.len(), 3);
    }

    fn text(text: &str) -> Typing {
        Typing::Text {
            text: text.into(),
            submit: false,
        }
    }

    #[test]
    fn text_in_the_active_layout_keeps_the_compositor_map() {
        assert_eq!(plan(&map(), 1, &text("ä€y")).unwrap().extended, None);
    }

    #[test]
    fn symbols_missing_from_the_layout_go_on_spare_keys_and_leave_every_key_unchanged() {
        let original = compile(&map()).unwrap();
        for group in [0, 1] {
            let planned = plan(&map(), group, &text("y→\u{301}")).unwrap();
            let extended = compile(planned.extended.as_deref().unwrap()).unwrap();
            assert_eq!(planned.keys.len(), 3);
            let spare = [60 - 8, 61 - 8];
            assert!(!spare.contains(&planned.keys[0].code.0));
            assert!(spare.contains(&planned.keys[1].code.0));
            assert!(spare.contains(&planned.keys[2].code.0));
            for raw in (8..=64).filter(|raw| !spare.contains(&(raw - 8))) {
                let code = xkb::Keycode::new(raw);
                assert_eq!(symbols(&original, code), symbols(&extended, code));
            }
        }
    }

    fn symbols(map: &xkb::Keymap, code: xkb::Keycode) -> Vec<Vec<xkb::Keysym>> {
        (0..map.num_layouts_for_key(code))
            .flat_map(|layout| {
                (0..map.num_levels_for_key(code, layout))
                    .map(move |level| map.key_get_syms_by_level(code, layout, level).to_vec())
            })
            .collect()
    }

    #[test]
    fn more_missing_symbols_than_spare_keys_refuse_the_whole_request() {
        assert!(plan(&map(), 0, &text("éß→")).is_err());
    }

    #[test]
    fn unknown_or_control_characters_and_missing_key_names_refuse() {
        for typing in [text("ok\u{7}"), Typing::Keys(vec!["eacute".into()])] {
            assert!(plan(&map(), 0, &typing).is_err());
        }
        assert!(plan(&map(), 2, &Typing::Keys(vec!["a".into()])).is_err());
    }
}
