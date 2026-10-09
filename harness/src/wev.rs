//! Reads back `wev`'s log. wev prints one line per Wayland event (`proxy_log` in wev
//! 1.1.0's `wev.c`), for example
//! `[        14:      wl_pointer] motion: time: 4001; x, y: 10.000000, 10.000000`.

pub(crate) mod keyboard;

/// One logged event: interface, event name and the rest of the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Event<'a> {
    pub(crate) interface: &'a str,
    pub(crate) name: &'a str,
    pub(crate) detail: &'a str,
}

/// Lines that don't start with `[`, such as the continuation of a `configure` line, are
/// skipped.
fn parse(line: &str) -> Option<Event<'_>> {
    let (id_and_interface, rest) = line.strip_prefix('[')?.split_once(']')?;
    let (_, interface) = id_and_interface.split_once(':')?;
    let rest = rest.trim_start();
    let (name, detail) = rest.split_once(": ").unwrap_or((rest, ""));
    Some(Event {
        interface: interface.trim(),
        name,
        detail,
    })
}

/// The log up to its last newline. wev may be part-way through writing the line after it.
fn complete(log: &str) -> &str {
    log.rfind('\n')
        .and_then(|end| log.get(..=end))
        .unwrap_or_default()
}

fn pointer_events(log: &str) -> impl Iterator<Item = Event<'_>> {
    complete(log)
        .lines()
        .filter_map(parse)
        .filter(|event| event.interface == "wl_pointer")
}

fn position(text: &str) -> Option<(f64, f64)> {
    let (x, y) = text.split_once(", ")?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

/// wev 1.1.0 prints Wayland's uint32 timestamps with `%d`.
pub(crate) fn time(detail: &str) -> crate::failure::Result<u32> {
    let value = detail
        .split_once("time: ")
        .and_then(|(_, rest)| rest.split([';', ' ', ',']).next());
    value
        .and_then(|value| {
            value.parse::<u32>().ok().or_else(|| {
                value
                    .parse::<i32>()
                    .ok()
                    .map(|signed| u32::from_ne_bytes(signed.to_ne_bytes()))
            })
        })
        .ok_or_else(|| crate::failure::Failure::new(format!("invalid wev time: {detail}")))
}

/// A `button` event's code and whether it was pressed.
fn button(event: Event<'_>) -> crate::failure::Result<(u32, bool)> {
    use crate::failure::Failure;
    let number = |field| {
        event
            .detail
            .split_once(field)
            .and_then(|(_, rest)| rest.split([';', ' ', ',']).next())
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(|| Failure::new(format!("invalid wev button: {}", event.detail)))
    };
    let state = number("state: ")?;
    if state > 1 {
        return Err(Failure::new(format!("invalid wev button state: {state}")));
    }
    Ok((number("button: ")?, state == 1))
}

/// What the pointer did, in log order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Pointer {
    /// It entered the surface or moved, to this surface-local position.
    At(f64, f64),
    Button {
        code: u32,
        pressed: bool,
    },
}

/// The pointer's positions and buttons, in order. Other pointer events are left out.
pub(crate) fn pointer_trace(log: &str) -> crate::failure::Result<Vec<Pointer>> {
    pointer_events(log)
        .filter_map(|event| match event.name {
            "enter" | "motion" => event
                .detail
                .split_once("x, y: ")
                .and_then(|(_, at)| position(at))
                .map(|(x, y)| Ok(Pointer::At(x, y))),
            "button" => {
                Some(button(event).map(|(code, pressed)| Pointer::Button { code, pressed }))
            }
            _ => None,
        })
        .collect()
}

/// Every complete pointer frame that holds an `axis` event, without its `frame` event.
pub(crate) fn axis_frames(log: &str) -> Vec<Vec<Event<'_>>> {
    let mut frames = Vec::new();
    let mut current = Vec::new();
    for event in pointer_events(log) {
        if event.name == "frame" {
            let frame = std::mem::take(&mut current);
            if frame.iter().any(|logged: &Event<'_>| logged.name == "axis") {
                frames.push(frame);
            }
        } else {
            current.push(event);
        }
    }
    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
[        14:      wl_pointer] enter: serial: 9; surface: 3, x, y: 100.000000, 100.000000
[        14:      wl_pointer] frame
[        14:      wl_pointer] motion: time: 4001; x, y: 10.000000, 9.996094
[        14:      wl_pointer] frame
[        12:    xdg_toplevel] configure: width: 400; height: 300
                      activated
[        14:      wl_pointer] button: serial: 11; time: 8000; button: 272 (left), state: 1 (pressed)
[        14:      wl_pointer] frame
[        14:      wl_pointer] button: serial: 12; time: 8000; button: 272 (left), state: 0 (released)
[        14:      wl_pointer] frame
[        14:      wl_pointer] axis_source: 0 (wheel)
[        14:      wl_pointer] axis_value120: axis: 0 (vertical), value120: 120
[        14:      wl_pointer] axis: time: 12000; axis: 0 (vertical), value: 15.000000
[        14:      wl_pointer] frame
";

    #[test]
    fn signed_timestamp_printing_preserves_wayland_bits() {
        for (printed, expected) in [
            ("0", 0),
            ("2147483647", 2_147_483_647),
            ("-2147483648", 1_u32 << 31),
            ("-1", u32::MAX),
            ("4294967295", u32::MAX),
        ] {
            assert_eq!(
                time(&format!("serial: 1; time: {printed}; button: 272")).unwrap(),
                expected
            );
        }
        for bad in ["-2147483649", "4294967296", "bad", ""] {
            assert!(time(&format!("time: {bad};")).is_err());
        }
    }

    #[test]
    fn parses_events_and_skips_continuation_lines() {
        let events: Vec<Event<'_>> = LOG.lines().filter_map(parse).collect();
        assert_eq!(events.len(), 13);
        assert_eq!(
            events.get(4),
            Some(&Event {
                interface: "xdg_toplevel",
                name: "configure",
                detail: "width: 400; height: 300",
            })
        );
        assert_eq!(events.get(1).map(|event| event.name), Some("frame"));
    }

    #[test]
    fn the_pointer_trace_keeps_positions_and_buttons_in_order() {
        assert_eq!(
            pointer_trace(LOG).unwrap(),
            [
                Pointer::At(100.0, 100.0),
                Pointer::At(10.0, 9.996_094),
                Pointer::Button {
                    code: 272,
                    pressed: true
                },
                Pointer::Button {
                    code: 272,
                    pressed: false
                },
            ]
        );
        let frames = axis_frames(LOG);
        assert_eq!(frames.len(), 1);
        let names: Vec<&str> = frames[0].iter().map(|event| event.name).collect();
        assert_eq!(names, ["axis_source", "axis_value120", "axis"]);
    }

    #[test]
    fn a_line_without_its_newline_is_not_read() {
        let partial = "[        14:      wl_pointer] motion: time: 4003; x, y: 10.000000, 2";
        assert_eq!(pointer_trace(partial).unwrap(), []);
        assert_eq!(
            pointer_trace(&format!("{partial}90.000000\n")).unwrap(),
            [Pointer::At(10.0, 290.0)]
        );
    }

    #[test]
    fn an_unfinished_axis_frame_is_not_returned() {
        let unfinished = LOG.trim_end().rsplit_once('\n').unwrap().0;
        assert_eq!(axis_frames(unfinished).len(), 0);
        assert_eq!(pointer_trace("").unwrap(), []);
    }
}
