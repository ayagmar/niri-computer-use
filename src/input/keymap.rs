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

/// What to type: the keys in consecutive parts, each typed with its own map.
#[derive(Debug)]
pub(super) struct Plan {
    pub(super) parts: Vec<Part>,
}

#[derive(Debug)]
pub(super) struct Part {
    pub(super) keys: Vec<Key>,
    /// The compositor's map with the symbols of this part the active layout lacks on
    /// spare keys, serialized by libxkbcommon as the compositor re-serializes it, to
    /// upload before the part's first key; `None` when the layout has them all.
    pub(super) extended: Option<String>,
}

impl Plan {
    /// Every key, in order.
    pub(super) fn keys(&self) -> impl Iterator<Item = &Key> {
        self.parts.iter().flat_map(|part| &part.keys)
    }

    /// Every key, in order, with its part's extended map on the part's first key.
    pub(super) fn strokes(self) -> impl Iterator<Item = (Option<String>, Key)> {
        self.parts.into_iter().flat_map(|part| {
            let mut extended = part.extended;
            part.keys.into_iter().map(move |key| (extended.take(), key))
        })
    }
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
    let (text, submit) = match typing {
        Typing::Keys(combos) => {
            let keys = combos
                .iter()
                .map(|combo| combo_key(&keymap, group, combo))
                .collect::<Result<_, _>>()?;
            return Ok(Plan {
                parts: vec![Part {
                    keys,
                    extended: None,
                }],
            });
        }
        Typing::Text { text, submit } => (text, *submit),
    };
    let spare = spare_keys(&keymap).len();
    let mut parts = split(&keymap, group, text, spare)?
        .into_iter()
        .map(|(part, missing)| text_part(map, &keymap, group, part, &missing))
        .collect::<Result<Vec<_>, _>>()?;
    if submit && let Some(last) = parts.last_mut() {
        // Every map of the call has `Return` on the compositor's key for it.
        last.keys.push(combo_key(&keymap, group, "Return")?);
    }
    Ok(Plan { parts })
}

/// `text`'s keys in `keymap`, extended with `missing` on spare keys when there are any.
fn text_part(
    map: &str,
    keymap: &xkb::Keymap,
    group: u32,
    text: &str,
    missing: &[xkb::Keysym],
) -> Result<Part, CallError> {
    if missing.is_empty() {
        return Ok(Part {
            keys: characters(keymap, group, text)?,
            extended: None,
        });
    }
    let (extended, serialized) = extend(map, keymap, missing)?;
    Ok(Part {
        keys: characters(&extended, group, text)?,
        extended: Some(serialized),
    })
}

fn characters(keymap: &xkb::Keymap, group: u32, text: &str) -> Result<Vec<Key>, CallError> {
    text.chars()
        .map(|c| {
            find(keymap, group, text_sym(c)).map(|mut key| {
                key.character = true;
                key
            })
        })
        .collect()
}

/// The keysym that types `c`. `Typing::check` has refused control characters, which
/// would be keys rather than characters.
fn text_sym(c: char) -> xkb::Keysym {
    xkb::utf32_to_keysym(u32::from(c))
}

/// `text` cut at character boundaries into consecutive parts, each with its distinct
/// printable symbols the active layout lacks, in order of use, at most `spare` of them.
/// A part ends right before a missing symbol it has no spare key left for; characters
/// the layout has never start one.
fn split<'t>(
    keymap: &xkb::Keymap,
    group: u32,
    text: &'t str,
    spare: usize,
) -> Result<Vec<(&'t str, Vec<xkb::Keysym>)>, CallError> {
    let mut parts = Vec::new();
    let (mut rest, mut start) = (text, 0);
    let mut missing = Vec::new();
    for (at, c) in text.char_indices() {
        let sym = text_sym(c);
        if find(keymap, group, sym).is_ok() || missing.contains(&sym) {
            continue;
        }
        if sym.raw() == 0 || c.is_control() {
            return Err(refused(
                "a control character is absent from the active compositor keymap; nothing was typed",
            ));
        }
        if spare == 0 {
            return Err(refused(
                "a symbol is absent from the active layout and the compositor keymap has no spare key for it; nothing was typed",
            ));
        }
        if missing.len() == spare {
            let (part, after) = rest.split_at(at - start);
            parts.push((part, std::mem::take(&mut missing)));
            (rest, start) = (after, at);
        }
        missing.push(sym);
    }
    parts.push((rest, missing));
    Ok(parts)
}

/// Puts each symbol on a named key that has no symbols, at most `MAX_X11_KEYCODE`, of
/// which `split` left enough, and proves every other key, the layouts and the modifiers
/// unchanged. Returns the extended map compiled, and as libxkbcommon serializes it.
fn extend(
    map: &str,
    keymap: &xkb::Keymap,
    syms: &[xkb::Keysym],
) -> Result<(xkb::Keymap, String), CallError> {
    let spare = spare_keys(keymap);
    if syms.len() > spare.len() {
        return Err(refused(
            "more symbols than spare keys for one extended keymap; nothing was typed",
        ));
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
    let text = extended.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    Ok((extended, text))
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

/// The first Unicode keysym: from here on, a keysym is a character's code point.
const UNICODE_KEYSYMS: u32 = 0x0100_0000;

/// Whether `combo` is modifiers a combination may hold and a keysym XKB knows by name, in
/// any case, as wtype looks keys up. A name in the `U<hex>` or `0x<hex>` form, or one for
/// a Unicode keysym, is left out: such names spell any text, one character each.
pub(crate) fn known_combo(combo: &str) -> bool {
    parse_combo(combo).is_ok_and(|(_, keysym_name)| {
        let keysym = xkb::keysym_from_name(keysym_name, xkb::KEYSYM_CASE_INSENSITIVE);
        keysym != xkb::Keysym::NoSymbol
            && keysym.raw() < UNICODE_KEYSYMS
            && !numeric_name(keysym_name)
    })
}

/// A keysym named by its number, `U` and a code point or `0x` and a keysym, in hex.
fn numeric_name(name: &str) -> bool {
    let digits = name
        .strip_prefix(['U', 'u'])
        .or_else(|| name.strip_prefix("0x"))
        .or_else(|| name.strip_prefix("0X"));
    digits.is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_hexdigit()))
}

/// `combo`'s key in the active layout `group`, or else in the first other layout of `map`
/// that has its keysym, sent in that layout with that layout's level modifiers: `ctrl+v`
/// pastes while a layout without Latin letters is active. The zero modifiers after the key
/// put the active layout back.
fn combo_key(map: &xkb::Keymap, group: u32, combo: &str) -> Result<Key, CallError> {
    let (modifiers, keysym_name) = parse_combo(combo).map_err(CallError::InvalidArguments)?;
    let sym = combo_keysym(keysym_name)?;
    let mut key = find(map, group, sym).or_else(|_| {
        (0..map.num_layouts())
            .filter(|&other| other != group)
            .find_map(|other| find(map, other, sym).ok())
            .ok_or_else(|| {
                refused(
                    "a requested key is absent from every layout of the compositor keymap; nothing was typed",
                )
            })
    })?;
    key.modifiers |= modifier_mask(map, &modifiers)?;
    Ok(key)
}

/// The keysym `name` names exactly, or else in any case, as wtype looks keys up: `escape`
/// is `Escape`, while `a` and `A` stay apart.
fn combo_keysym(name: &str) -> Result<xkb::Keysym, CallError> {
    [xkb::KEYSYM_NO_FLAGS, xkb::KEYSYM_CASE_INSENSITIVE]
        .into_iter()
        .map(|flags| xkb::keysym_from_name(name, flags))
        .find(|&sym| sym != xkb::Keysym::NoSymbol)
        .ok_or_else(|| refused("unknown keysym; nothing was typed"))
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

    /// The keys of `typing` in `group`, which needs one map.
    fn keys(group: u32, typing: &Typing) -> Vec<Key> {
        let [part] = <[Part; 1]>::try_from(plan(&map(), group, typing).unwrap().parts).unwrap();
        part.keys
    }

    #[test]
    fn text_uses_the_active_layout_and_its_levels() {
        let typing = Typing::Text {
            text: "yZ".into(),
            submit: false,
        };
        let us = keys(0, &typing);
        let de = keys(1, &typing);
        assert_ne!(us[0].code, de[0].code);
        assert_eq!(us[0].modifiers, 0);
        assert_ne!(us[1].modifiers, 0);
        let umlaut = Typing::Text {
            text: "ä€".into(),
            submit: true,
        };
        assert_eq!(keys(1, &umlaut).len(), 3);
    }

    fn text(text: &str) -> Typing {
        Typing::Text {
            text: text.into(),
            submit: false,
        }
    }

    #[test]
    fn text_in_the_active_layout_keeps_the_compositor_map() {
        let parts = plan(&map(), 1, &text("ä€y")).unwrap().parts;
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].extended, None);
    }

    #[test]
    fn symbols_missing_from_the_layout_go_on_spare_keys_and_leave_every_key_unchanged() {
        let original = compile(&map()).unwrap();
        for group in [0, 1] {
            let [planned] =
                <[Part; 1]>::try_from(plan(&map(), group, &text("y→\u{301}")).unwrap().parts)
                    .unwrap();
            let extended = compile(planned.extended.as_deref().unwrap()).unwrap();
            assert_eq!(planned.keys.len(), 3);
            assert!(!SPARE.contains(&planned.keys[0].code.0));
            assert!(SPARE.contains(&planned.keys[1].code.0));
            assert!(SPARE.contains(&planned.keys[2].code.0));
            for raw in (8..=64).filter(|raw| !SPARE.contains(&(raw - 8))) {
                let code = xkb::Keycode::new(raw);
                assert_eq!(symbols(&original, code), symbols(&extended, code));
            }
        }
    }

    /// The fixture's spare keys, `<I60>` and `<I61>`, as evdev codes.
    const SPARE: [u32; 2] = [60 - 8, 61 - 8];

    fn symbols(map: &xkb::Keymap, code: xkb::Keycode) -> Vec<Vec<xkb::Keysym>> {
        (0..map.num_layouts_for_key(code))
            .flat_map(|layout| {
                (0..map.num_levels_for_key(code, layout))
                    .map(move |level| map.key_get_syms_by_level(code, layout, level).to_vec())
            })
            .collect()
    }

    #[test]
    fn as_many_missing_symbols_as_spare_keys_take_one_map() {
        let parts = plan(&map(), 0, &text("é→é→a")).unwrap().parts;
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].keys.len(), 5);
        assert!(parts[0].extended.is_some());
    }

    /// `ß` finds both spare keys taken by `é` and `→`, so a second map starts with it and
    /// puts it on the key `é` had; `a` and `y`, which the layout has, stay where they are.
    #[test]
    fn a_missing_symbol_past_the_spare_keys_starts_the_next_map() {
        let typing = Typing::Text {
            text: "éa→yßz".into(),
            submit: true,
        };
        let parts = plan(&map(), 0, &typing).unwrap().parts;
        let lengths: Vec<usize> = parts.iter().map(|part| part.keys.len()).collect();
        assert_eq!(lengths, [4, 3], "éa→y, then ßz and Return");
        assert_ne!(parts[0].extended, parts[1].extended);
        assert!(parts[1].extended.is_some());
        assert_eq!(parts[0].keys[0].code, parts[1].keys[0].code);
        let enter = parts[1].keys[2];
        assert!(!enter.character && !SPARE.contains(&enter.code.0));
    }

    /// However many symbols the layout lacks, every character is typed once, in order,
    /// and each map holds as many of them as there are spare keys; the last `é` needs a
    /// map of its own, since the first one's map is gone by then.
    #[test]
    fn every_character_goes_out_once_in_maps_of_at_most_the_spare_keys() {
        let text = "é←ß↑→↓é";
        let parts = plan(&map(), 0, &self::text(text)).unwrap().parts;
        assert_eq!(parts.len(), 4);
        let keys: Vec<Key> = parts.iter().flat_map(|part| part.keys.clone()).collect();
        assert_eq!(keys.len(), text.chars().count());
        assert!(
            keys.iter()
                .all(|key| key.character && SPARE.contains(&key.code.0))
        );
    }

    fn combo(group: u32, combo: &str) -> Key {
        let [key] = <[Key; 1]>::try_from(keys(group, &Typing::Keys(vec![combo.into()]))).unwrap();
        key
    }

    /// `z` is in both layouts, on another key in each, so it goes out in the active one;
    /// `a` is only in the first, so with the second active it goes out in the first, with
    /// Ctrl as asked.
    #[test]
    fn a_combination_goes_out_in_the_active_layout_or_else_one_that_has_its_keysym() {
        let compiled = compile(&map()).unwrap();
        let ctrl = modifier_mask(&compiled, &["ctrl"]).unwrap();
        let z = combo(1, "ctrl+z");
        assert_eq!((z.code.0, z.group, z.modifiers), (29 - 8, 1, ctrl));
        assert_eq!(combo(0, "ctrl+z").code.0, 52 - 8);
        let a = combo(1, "ctrl+a");
        assert_eq!((a.code.0, a.group, a.modifiers), (48 - 8, 0, ctrl));
    }

    /// `EuroSign` is the second layout's Shift level of `<U>`: the Shift the key needs is
    /// that layout's, though the first is active.
    #[test]
    fn a_combination_from_another_layout_takes_that_layouts_level_modifiers() {
        let shift = modifier_mask(&compile(&map()).unwrap(), &["shift"]).unwrap();
        let euro = combo(0, "EuroSign");
        assert_eq!(
            (euro.code.0, euro.group, euro.modifiers),
            (48 - 8, 1, shift)
        );
    }

    /// Key names go in any case, as wtype took them, but an exact name wins: `a` and `A`
    /// stay two keysyms.
    #[test]
    fn a_key_name_resolves_in_any_case_unless_it_names_another_keysym() {
        assert_eq!(combo_keysym("escape").unwrap(), xkb::Keysym::Escape);
        assert_eq!(combo_keysym("backspace").unwrap(), xkb::Keysym::BackSpace);
        assert_eq!(combo(0, "ctrl+return").code.0, 36 - 8);
        let shift = modifier_mask(&compile(&map()).unwrap(), &["shift"]).unwrap();
        assert_eq!(combo(0, "a").modifiers, 0);
        assert_eq!(combo(0, "A").modifiers, shift);
    }

    #[test]
    fn a_name_that_is_no_keysym_is_refused_as_unknown() {
        let keys = Typing::Keys(vec!["Nonsense".into()]);
        let Err(CallError::Tool(error)) = plan(&map(), 0, &keys) else {
            panic!("a name that is no keysym was planned");
        };
        assert!(error.detail.contains("unknown keysym"), "{}", error.detail);
    }

    /// The refusal names every layout, so it doesn't read as a layout switch away.
    #[test]
    fn a_combination_no_layout_has_is_refused_as_absent_from_all() {
        let keys = Typing::Keys(vec!["eacute".into()]);
        let Err(CallError::Tool(error)) = plan(&map(), 1, &keys) else {
            panic!("a keysym in no layout was planned");
        };
        assert!(
            error.detail.contains("absent from every layout"),
            "{}",
            error.detail
        );
    }

    #[test]
    fn an_extension_never_takes_more_symbols_than_spare_keys() {
        let compiled = compile(&map()).unwrap();
        let syms = "éß→".chars().map(text_sym).collect::<Vec<_>>();
        let Err(CallError::Tool(error)) = extend(&map(), &compiled, &syms) else {
            panic!("three symbols went on two spare keys");
        };
        assert!(
            error.detail.contains("more symbols than spare keys"),
            "{}",
            error.detail
        );
    }

    #[test]
    fn unknown_or_control_characters_and_missing_key_names_refuse() {
        for typing in [text("ok\u{7}"), Typing::Keys(vec!["eacute".into()])] {
            assert!(plan(&map(), 0, &typing).is_err());
        }
        assert!(plan(&map(), 2, &Typing::Keys(vec!["a".into()])).is_err());
    }
}
