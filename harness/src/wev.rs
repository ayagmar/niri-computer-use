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

/// The surface-local position of each `wl_pointer.enter`.
pub(crate) fn enters(log: &str) -> Vec<(f64, f64)> {
    pointer_events(log)
        .filter(|event| event.name == "enter")
        .filter_map(|event| position(event.detail.split_once("x, y: ")?.1))
        .collect()
}

fn position(text: &str) -> Option<(f64, f64)> {
    let (x, y) = text.split_once(", ")?;
    Some((x.parse().ok()?, y.parse().ok()?))
}

/// The surface-local position of the `motion` event sent with `time`.
pub(crate) fn motion(log: &str, time: u32) -> Option<(f64, f64)> {
    let prefix = format!("time: {time}; x, y: ");
    pointer_events(log)
        .filter(|event| event.name == "motion")
        .find_map(|event| position(event.detail.strip_prefix(&prefix)?))
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

/// What follows `button: ` in each `button` event sent with `time`, in order.
pub(crate) fn buttons(log: &str, time: u32) -> Vec<&str> {
    let marker = format!("; time: {time}; button: ");
    pointer_events(log)
        .filter(|event| event.name == "button")
        .filter_map(|event| Some(event.detail.split_once(&marker)?.1))
        .collect()
}

/// A complete button record, including its time, for the interrupted-pointer checks.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Button {
    pub(crate) time: u32,
    pub(crate) code: u32,
    pub(crate) pressed: bool,
}

pub(crate) fn button_trace(log: &str) -> crate::failure::Result<Vec<Button>> {
    use crate::failure::Failure;
    pointer_events(log)
        .filter(|event| event.name == "button")
        .map(|event| {
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
            Ok(Button {
                time: time(event.detail)?,
                code: number("button: ")?,
                pressed: state == 1,
            })
        })
        .collect()
}

/// The pointer events of the complete frame that holds the `axis` event sent with `time`.
pub(crate) fn axis_frame(log: &str, time: u32) -> Option<Vec<Event<'_>>> {
    let prefix = format!("time: {time};");
    let pointer: Vec<Event<'_>> = pointer_events(log).collect();
    let axis = pointer
        .iter()
        .position(|event| event.name == "axis" && event.detail.starts_with(&prefix))?;
    let (before, from_axis) = pointer.split_at(axis);
    let start = before
        .iter()
        .rposition(|event| event.name == "frame")
        .map_or(0, |frame| frame + 1);
    let end = axis + from_axis.iter().position(|event| event.name == "frame")?;
    pointer.get(start..end).map(<[Event<'_>]>::to_vec)
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
        let buttons =
            button_trace("[ 1: wl_pointer] button: time: -1; button: 272, state: 1 (pressed)\n")
                .unwrap();
        assert_eq!(buttons[0].time, u32::MAX);
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
    fn finds_pointer_events_by_time() {
        assert_eq!(enters(LOG), [(100.0, 100.0)]);
        assert_eq!(motion(LOG, 4001), Some((10.0, 9.996_094)));
        assert_eq!(motion(LOG, 4002), None);
        assert_eq!(
            buttons(LOG, 8000),
            [
                "272 (left), state: 1 (pressed)",
                "272 (left), state: 0 (released)"
            ]
        );
        assert_eq!(buttons(LOG, 9000), Vec::<&str>::new());
    }

    #[test]
    fn button_trace_keeps_state_time_and_rejects_malformed_records() {
        let seen = button_trace(LOG).unwrap();
        assert_eq!(
            seen,
            [
                Button {
                    time: 8000,
                    code: 272,
                    pressed: true
                },
                Button {
                    time: 8000,
                    code: 272,
                    pressed: false
                }
            ]
        );
        assert!(button_trace(&LOG.replace("state: 1", "state: 2")).is_err());
        assert!(button_trace("[ 1: wl_pointer] button: time: 1; button: bad, state: 0\n").is_err());
        assert_eq!(
            button_trace("[ 1: wl_pointer] button: time: 1; button: 272, state: 0").unwrap(),
            []
        );
    }

    #[test]
    fn axis_frame_spans_from_the_previous_frame_to_the_next() {
        let frame = axis_frame(LOG, 12000).unwrap();
        let names: Vec<&str> = frame.iter().map(|event| event.name).collect();
        assert_eq!(names, ["axis_source", "axis_value120", "axis"]);
        assert_eq!(axis_frame(LOG, 1), None);
    }

    #[test]
    fn a_line_without_its_newline_is_not_read() {
        let partial = "[        14:      wl_pointer] motion: time: 4003; x, y: 10.000000, 2";
        assert_eq!(motion(partial, 4003), None);
        assert_eq!(
            motion(&format!("{partial}90.000000\n"), 4003),
            Some((10.0, 290.0))
        );
    }

    #[test]
    fn an_unfinished_axis_frame_is_not_returned() {
        let unfinished = LOG.trim_end().rsplit_once('\n').unwrap().0;
        assert_eq!(axis_frame(unfinished, 12000), None);
        assert_eq!(enters(""), []);
    }
}
