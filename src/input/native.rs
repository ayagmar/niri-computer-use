//! Native per-key input. A dirty marker covers every possible press until release is
//! acknowledged and, after an extended keymap, niri has sent the compositor's map back.
//! SIGKILL requires recover; destruction alone releases nothing in niri. A paste's
//! `Aftercare` is armed right before the first stroke and told the key went out once the
//! release is acknowledged, before the marker comes off; a release niri never acknowledged
//! drops it, so the keeper keeps the pasted text.

use std::time::Duration;

use tokio::time::Instant;

use super::keyboard::{
    Expect, Sent, Typing, WTYPE_DEADLINE, check_expect, ended, partly, typed_so_far,
};
use super::keymap::{Key, Plan, plan};
use super::paste::Aftercare;
use super::{Input, focused_app_id};
use crate::act::{self, Observed, Outcome};
use crate::control::cleanup;
use crate::control::desk::Desk;
use crate::control::marker::{self, Marker, Native, Written};
use crate::error::{CallError, ErrorName, ToolError};
use crate::niri::{
    self, Socket,
    keyboard::Keyboard,
    waiter::{View, Waited, Waiter},
};
use crate::policy;

/// Past how long a key loop that found the stop flag set stops waiting for the desk's stop
/// watcher, which reads the flag on every change in the runtime directory and once a
/// second.
const STOP_SEEN: Duration = Duration::from_secs(2);

pub(super) async fn type_input(
    input: Input<'_>,
    typing: Typing,
    expect: Expect,
    aftercare: &mut Option<Aftercare>,
) -> Result<Outcome, CallError> {
    let mut waiter = niri::waiter(input.niri.events).await?;
    let focus = check_expect(&expect, waiter.view())?;
    if let Some(refused) = policy::refuse_input(input.policy, focused_app_id(waiter.view())) {
        return Err(refused.into());
    }
    let (keyboard, group, plan) = bind(input, waiter.view(), &typing).await?;
    let keys = plan.keys;
    let revision = keyboard.revision();
    let before = waiter.view().focused_window();
    let mut device = Device::new(keyboard, input, &typing, &keys, group).await?;
    if let Some(map) = &plan.extended {
        device.keyboard()?.extend(map)?;
    }
    let call = Call {
        typing: &typing,
        budget: Budget::start(keys.len()),
        marks: aftercare.is_none(),
    };
    let mut sent = Sent::default();
    for key in keys {
        act::fresh_lock(input.niri)
            .await
            .map_err(|error| partly(error, &typing, sent))?;
        // niri's events since the lock was read or the last key, right before this one.
        if let Some(outcome) = interrupted(&mut waiter, before, group).await {
            device.finish(waiter.view().keyboard_group()).await?;
            return Ok(ended(outcome, focus, &typing, sent));
        }
        may_press(input, waiter.view(), &call, sent).await?;
        device.commit(aftercare, input.niri.desk).await?;
        // Sent to niri before it failed or ran out of time, the key may still have gone out.
        call.budget
            .bound(device.stroke(key))
            .await
            .map_err(|error| partly(error, &typing, sent.underway()))?;
        sent.add_key(key.character);
        // Counts only once `may_press` told the desk the key was going out.
        let answered = typed_so_far(waiter.view(), focus, &typing, sent);
        input.niri.desk.answered(answered);
        if device.revision()? != revision {
            device.finish(waiter.view().keyboard_group()).await?;
            return Ok(ended(keymap_changed(waiter.view()), focus, &typing, sent));
        }
    }
    device.finish(waiter.view().keyboard_group()).await?;
    let outcome = interrupted(&mut waiter, before, group)
        .await
        .unwrap_or_else(|| Outcome::seen(Observed::Sent, waiter.view(), Vec::new()));
    Ok(ended(outcome, focus, &typing, sent))
}

/// Binds a virtual keyboard on niri's display and plans `typing`'s keys in the layout
/// niri has active, which it returns too.
async fn bind(
    input: Input<'_>,
    view: &View,
    typing: &Typing,
) -> Result<(Keyboard, u32, Plan), CallError> {
    let display = input.display.path().map_err(Clone::clone)?;
    let keyboard = Keyboard::bind(display, niri::pid(input.niri.socket).await?).await?;
    let group = view.keyboard_group().ok_or_else(|| {
        ToolError::new(
            ErrorName::NiriUnavailable,
            "the active keyboard layout is unknown",
        )
    })?;
    let plan = plan(keyboard.map()?, group, typing)?;
    Ok((keyboard, group, plan))
}

/// The outcome once the compositor's keymap changed while keys went out.
fn keymap_changed(view: &View) -> Outcome {
    Outcome::uncertain(
        Some(true),
        Some(view),
        "the compositor keymap changed during typing".into(),
    )
}

/// What stays the same for each key of a call.
struct Call<'a> {
    typing: &'a Typing,
    budget: Budget,
    /// Whether to tell the desk each key is going out. `paste`'s keeper reports whether
    /// its key went out instead.
    marks: bool,
}

/// Whether the next key may go out: the stop flag isn't set, the focused app in `view` isn't
/// denied, the call's time isn't up, and the lock state was read at most `LAST_GATE` ago.
/// With `marks`, tells the desk the key is going out.
async fn may_press(
    input: Input<'_>,
    view: &View,
    call: &Call<'_>,
    sent: Sent,
) -> Result<(), ToolError> {
    if input
        .runtime
        .stopped()
        .map_err(|error| upstream(&format!("read stop flag: {error}")))?
    {
        if call.marks && !sent.nothing() {
            // Keys went out, so the call ends `uncertain`, as the desk's stop watcher ends
            // it once it sees the same flag.
            tokio::time::sleep(STOP_SEEN).await;
        }
        return Err(ToolError::new(
            ErrorName::Stopped,
            format!(
                "stopped after {} characters and {} keys",
                sent.chars, sent.keys
            ),
        ));
    }
    if let Some(refused) = policy::refuse_input(input.policy, focused_app_id(view)) {
        return Err(partly(refused, call.typing, sent));
    }
    call.budget
        .check()
        .map_err(|error| partly(error, call.typing, sent))?;
    let unlocked = if call.marks {
        input.niri.desk.sending_input()
    } else {
        input.niri.desk.fresh()
    };
    unlocked.map_err(|error| partly(error, call.typing, sent))
}

async fn interrupted(waiter: &mut Waiter, before: Option<u64>, group: u32) -> Option<Outcome> {
    match waiter
        .until(Duration::ZERO, |view| {
            (view.focused_window() != before || view.keyboard_group() != Some(group)).then_some(())
        })
        .await
    {
        Waited::Timeout => None,
        Waited::Done(()) => Some(Outcome::seen(
            Observed::Interrupted,
            waiter.view(),
            Vec::new(),
        )),
        Waited::Lost(reason) => Some(Outcome::uncertain(Some(true), Some(waiter.view()), reason)),
    }
}

/// A key's share of its call's time: three frames at 60 Hz. The slowest steady rate
/// measured, a nested niri drawing every frame, was two (32 ms).
const PER_KEY: Duration = Duration::from_millis(50);

/// The keys' time: `PER_KEY` for each key, and at least what one `wtype` call gets, from
/// before the first key until niri has acknowledged the last. Each round trip has its own
/// deadline, but a niri that answers every one late could otherwise stretch a call
/// without end. Binding and planning before have their round trips' deadlines, and the
/// release after runs within the cleanup's. A call out of time stops, before its next key
/// or while one waits for niri, and the dropped device's cleanup releases the keys.
struct Budget {
    length: Duration,
    ends: Instant,
}

impl Budget {
    fn start(keys: usize) -> Self {
        let keys = u32::try_from(keys).unwrap_or(u32::MAX);
        let length = PER_KEY.saturating_mul(keys).max(WTYPE_DEADLINE);
        Self {
            length,
            ends: Instant::now() + length,
        }
    }

    /// Refuses to start another key once the time is up.
    fn check(&self) -> Result<(), ToolError> {
        if Instant::now() < self.ends {
            return Ok(());
        }
        Err(self.over(""))
    }

    /// Runs a key's `stroke` until the time is up.
    async fn bound(
        &self,
        stroke: impl Future<Output = Result<(), ToolError>>,
    ) -> Result<(), ToolError> {
        tokio::time::timeout_at(self.ends, stroke)
            .await
            .unwrap_or_else(|_| Err(self.over(": niri hadn't acknowledged the key last sent")))
    }

    fn over(&self, pending: &str) -> ToolError {
        ToolError::new(
            ErrorName::DeadlineExceeded,
            format!(
                "native typing took longer than its {:?}{pending}",
                self.length
            ),
        )
    }
}

struct Device {
    keyboard: Option<Keyboard>,
    marker: Option<Written>,
    /// niri's socket, asked for the active layout before the layout is restored.
    socket: Socket,
    /// The layout niri last reported to this call.
    group: u32,
    aftercare: Option<Aftercare>,
    /// The deadline of the cleanup task the release runs in, which a cleanup started when
    /// the device drops takes over.
    ends: Option<Instant>,
}

impl Device {
    async fn new(
        keyboard: Keyboard,
        input: Input<'_>,
        typing: &Typing,
        keys: &[Key],
        group: u32,
    ) -> Result<Self, ToolError> {
        let mut codes: Vec<u32> = keys.iter().map(|key| key.code.0).collect();
        codes.sort_unstable();
        codes.dedup();
        let mut marker = Marker::pending(typing.tool(), Vec::new());
        marker.keyboard = Some(Native {
            codes: codes.clone(),
            group,
        });
        let marker = Written::write(input.runtime, marker)
            .await
            .map_err(|error| marker::unwritten(&error))?;
        Ok(Self {
            keyboard: Some(keyboard),
            marker: Some(marker),
            socket: input.niri.socket.clone(),
            group,
            aftercare: None,
            ends: None,
        })
    }

    fn keyboard(&mut self) -> Result<&mut Keyboard, ToolError> {
        self.keyboard
            .as_mut()
            .ok_or_else(|| upstream("native keyboard is gone"))
    }

    fn revision(&mut self) -> Result<u64, ToolError> {
        Ok(self.keyboard()?.revision())
    }

    /// Takes `aftercare`, if it is still there, and arms it.
    async fn commit(
        &mut self,
        aftercare: &mut Option<Aftercare>,
        desk: &Desk,
    ) -> Result<(), ToolError> {
        let Some(taken) = aftercare.take() else {
            return Ok(());
        };
        self.aftercare = Some(taken.arm(desk).await?);
        Ok(())
    }

    async fn stroke(&mut self, key: Key) -> Result<(), ToolError> {
        let keyboard = self.keyboard()?;
        keyboard.modifiers(key.modifiers, key.group)?;
        keyboard.key(key.code.0, true)?;
        keyboard.key(key.code.0, false)?;
        keyboard.modifiers(0, key.group)?;
        keyboard.sync().await
    }

    /// Releases everything and puts the latest compositor keymap back in the layout niri
    /// has active, or without niri's answer in the one its event stream last reported,
    /// `group`, if it reported one: a layout the user switched to meanwhile stays. Then
    /// tells a paste's keeper the key went out and clears the marker. That runs in a
    /// cleanup task, within the cleanup's deadline, so a dropped call can't cut it short or
    /// leave the keeper without its `p`.
    async fn finish(mut self, group: Option<u32>) -> Result<(), ToolError> {
        if let Some(group) = group {
            self.group = group;
        }
        let aftercare = self.aftercare.take();
        let ends = cleanup::deadline();
        self.ends = Some(ends);
        let finished = cleanup::spawn(ends, async move {
            self.release().await?;
            if let Some(aftercare) = aftercare {
                aftercare.sent().await;
            }
            self.clear().await
        });
        finished
            .await
            .map_err(|error| upstream(&format!("the release task ended: {error}")))?
    }

    async fn release(&mut self) -> Result<(), ToolError> {
        if let Ok(group) = niri::keyboard_group(&self.socket).await {
            self.group = group;
        }
        let group = self.group;
        let keyboard = self.keyboard()?;
        keyboard.release(&[], group).await?;
        keyboard.restore(group).await
    }

    async fn clear(&mut self) -> Result<(), ToolError> {
        if let Some(marker) = self.marker.take() {
            marker
                .clear()
                .await
                .map_err(|error| upstream(&format!("clear native input-dirty marker: {error}")))?;
        }
        Ok(())
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let (Some(mut keyboard), Some(marker)) = (self.keyboard.take(), self.marker.take()) else {
            return;
        };
        if keyboard.release_now(&[], self.group).is_err() {
            return;
        }
        if tokio::runtime::Handle::try_current().is_ok() {
            let aftercare = self.aftercare.take();
            cleanup::spawn(
                self.ends.unwrap_or_else(cleanup::deadline),
                acknowledge_release(
                    keyboard,
                    marker,
                    aftercare,
                    (self.socket.clone(), self.group),
                ),
            );
        }
    }
}

/// Once niri has acknowledged the release, which went out at once in the layout the call
/// last knew, `layout`'s group, puts the latest compositor keymap back in the layout niri
/// has active by now, and then, after telling a paste's keeper the key went out, clears
/// the marker. Unacknowledged or not restored, the marker stays, and the keeper, dropped,
/// keeps the pasted text.
async fn acknowledge_release(
    mut keyboard: Keyboard,
    marker: Written,
    aftercare: Option<Aftercare>,
    layout: (Socket, u32),
) -> Result<(), ToolError> {
    keyboard.sync().await?;
    restore_active(&mut keyboard, &layout.0, layout.1).await?;
    if let Some(aftercare) = aftercare {
        aftercare.sent().await;
    }
    marker
        .clear()
        .await
        .map_err(|error| upstream(&format!("clear native input-dirty marker: {error}")))
}

/// Puts the latest base map back with zero modifiers in the layout niri has active, or
/// without niri's answer in `sent`, and sends the modifiers once more after niri has taken
/// it: in the nested trials, this device saw the map a dropped call restored come back
/// only after a further input, and the client sometimes got no modifiers event for one of
/// the two. Then proves clients and the device hold the latest base map; an error leaves
/// the caller's marker for `recover`. A held pointer modifier's release ends the same way.
pub(super) async fn restore_active(
    keyboard: &mut Keyboard,
    socket: &Socket,
    sent: u32,
) -> Result<(), ToolError> {
    let group = niri::keyboard_group(socket).await.unwrap_or(sent);
    keyboard.put_back(group).await?;
    keyboard.modifiers(0, group)?;
    keyboard.sync().await?;
    if keyboard.restored() {
        return Ok(());
    }
    Err(upstream(
        "the last keymap niri sent clients isn't its latest compositor keymap",
    ))
}

fn upstream(detail: &str) -> ToolError {
    ToolError::new(ErrorName::UpstreamError, detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::runtime::RuntimeDir;
    use crate::input::keyboard::Typing;
    use std::path::Path;

    use crate::act::Niri;
    use crate::control::LockSources;
    use crate::error::CallError;
    use crate::input::paste::{
        Clipboard, Report, SLOW_ADMISSION, STOPPED_WAITING, desk_read, fake_aftercare,
    };
    use crate::noctalia;
    use crate::policy::Loaded;
    use crate::test_support::{
        fresh_dir,
        wayland::{APP_ID, Compositor},
    };
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const BASE: &str = include_str!("../../tests/fixtures/us-de.xkb");
    /// evdev codes in the fixture's first layout: `a`, `z` and Shift.
    const A: u32 = 40;
    const Z: u32 = 44;
    const HELD: u32 = 42;

    /// Each key's press, then its release.
    fn presses(keys: &[Key]) -> Vec<(u32, bool)> {
        keys.iter()
            .flat_map(|key| [(key.code.0, true), (key.code.0, false)])
            .collect()
    }

    /// A device on the fake niri in `dir`, with `text` planned in layout 0: its extended
    /// keymap uploaded, if the layout lacks some of the text's symbols.
    async fn typing(niri: &Compositor, text: &str) -> (Device, Vec<Key>) {
        let keyboard = Keyboard::bind(niri.display(), std::process::id())
            .await
            .unwrap();
        let typing = Typing::Text {
            text: text.to_owned(),
            submit: false,
        };
        let plan = plan(keyboard.map().unwrap(), 0, &typing).unwrap();
        let mut device = Device {
            keyboard: Some(keyboard),
            marker: None,
            socket: niri.niri_socket(),
            group: 0,
            aftercare: None,
            ends: None,
        };
        if let Some(map) = &plan.extended {
            device.keyboard().unwrap().extend(map).unwrap();
        }
        (device, plan.keys)
    }

    /// Text beyond the layout goes out through an extended keymap, which clients then hold;
    /// the call ends with every key released, zero modifiers and the compositor's keymap back
    /// in clients and active in niri, so the user's next key sends no keymap.
    #[tokio::test]
    async fn a_call_with_an_extended_keymap_ends_with_the_compositors_back() {
        let dir = fresh_dir("native-extension");
        let niri = Compositor::start(&dir, BASE);
        let (mut device, keys) = typing(&niri, "é").await;
        let pairs = presses(&keys);
        assert_eq!(pairs.len(), 2);
        for key in keys {
            device.stroke(key).await.unwrap();
        }
        let during = niri.seen();
        assert_eq!(during.keys, pairs, "each key pressed and released once");
        assert_ne!(
            during.active, during.compositor,
            "the extension reached clients"
        );
        device.finish(Some(0)).await.unwrap();
        let seen = niri.seen();
        assert_eq!(seen.active, seen.compositor);
        assert!(seen.held_keys().is_empty(), "{:?}", seen.keys);
        assert_eq!(seen.last_modifiers(), Some((0, 0)));
        niri.physical(38).await;
        assert_eq!(
            niri.seen().keymaps,
            seen.keymaps,
            "a physical key sent a keymap"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A call dropped mid-text, as a cancellation drops it, releases at once and, once niri
    /// has acknowledged that, puts the compositor's keymap back in the layout niri has active
    /// before its marker comes off.
    #[tokio::test]
    async fn a_dropped_call_restores_the_compositors_keymap_before_its_marker_goes() {
        let dir = fresh_dir("native-dropped");
        let niri = Compositor::start(&dir, BASE);
        let runtime = RuntimeDir::of(&niri.env(&dir)).unwrap();
        runtime.create().unwrap();
        let (mut device, keys) = typing(&niri, "éa").await;
        device.marker = Some(
            Written::write(&runtime, Marker::pending("type_text", Vec::new()))
                .await
                .unwrap(),
        );
        device.stroke(keys[0]).await.unwrap();
        // A key still down when the call drops, as between a press and its release.
        let keyboard = device.keyboard().unwrap();
        keyboard.key(HELD, true).unwrap();
        keyboard.sync().await.unwrap();
        assert_eq!(niri.seen().held_keys(), [HELD]);
        niri.switch(1).await;
        drop(device);
        let until = Instant::now() + cleanup::LIMIT;
        while marker::read(&runtime).is_some() {
            assert!(Instant::now() < until, "the marker stayed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let seen = niri.seen();
        assert_eq!(seen.active, seen.compositor);
        assert_eq!(seen.keys.last(), Some(&(HELD, false)), "{:?}", seen.keys);
        assert!(seen.held_keys().is_empty(), "{:?}", seen.keys);
        assert_eq!(
            seen.last_modifiers(),
            Some((0, 1)),
            "not in niri's active layout"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Serves Noctalia's status for `env`, with the screen unlocked, to every read.
    fn unlocked(env: &crate::Env) {
        let listener = tokio::net::UnixListener::bind(noctalia::socket(env).unwrap()).unwrap();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                stream.read_to_end(&mut request).await.ok();
                stream.write_all(br#"{"locked": false}"#).await.ok();
            }
        });
    }

    /// Types `text` through the entry point with `expect` on the fake's focused window,
    /// with the screen unlocked.
    async fn type_through(niri: &Compositor, dir: &Path, text: &str) -> Result<Outcome, CallError> {
        let env = crate::Env {
            wayland_display: Some("fake-noctalia".into()),
            ..niri.env(dir)
        };
        unlocked(&env);
        let runtime = RuntimeDir::of(&env).unwrap();
        runtime.create().unwrap();
        let socket = niri.niri_socket();
        let events = niri.events();
        let desk = Desk::start(&env);
        let input = Input {
            niri: Niri {
                socket: &socket,
                events: Ok(&events),
                desk: &desk,
                locks: LockSources {
                    env: &env,
                    noctalia: true,
                },
            },
            display: &env.display,
            runtime: &runtime,
            policy: &Loaded::Missing,
            keyboard: None,
            a11y: None,
        };
        let typing = Typing::Text {
            text: text.to_owned(),
            submit: false,
        };
        type_input(input, typing, Expect::App(APP_ID.to_owned()), &mut None).await
    }

    /// Waits until the call's marker in `dir` is gone, as the cleanup's deadline allows.
    async fn marker_cleared(niri: &Compositor, dir: &Path) {
        let runtime = RuntimeDir::of(&niri.env(dir)).unwrap();
        let until = Instant::now() + cleanup::LIMIT;
        while marker::read(&runtime).is_some() {
            assert!(Instant::now() < until, "the marker stayed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_call_types_each_key_once_and_clears_its_marker() {
        let dir = fresh_dir("native-entry");
        let niri = Compositor::start(&dir, BASE);
        let outcome = type_through(&niri, &dir, "az").await.unwrap();
        assert_eq!(outcome.observed, Observed::Sent);
        assert_eq!(
            niri.seen().keys,
            [(A, true), (A, false), (Z, true), (Z, false)]
        );
        marker_cleared(&niri, &dir).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Each key acknowledged in 1.9 s, inside a round trip's two seconds, but the second
    /// overruns the two keys' three: the call ends `deadline_exceeded` while niri still
    /// holds it, says what it typed, and the cleanup releases everything.
    #[tokio::test]
    async fn a_call_whose_last_key_overruns_its_time_ends_and_is_released() {
        let dir = fresh_dir("native-overrun");
        let niri = Compositor::start(&dir, BASE);
        niri.slow_keys(Duration::from_millis(1900)).await;
        let started = Instant::now();
        let Err(CallError::Tool(error)) = type_through(&niri, &dir, "az").await else {
            panic!("the call ended in time");
        };
        assert!(
            started.elapsed() < Duration::from_millis(3500),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
        assert!(
            error
                .detail
                .contains("typed 1 of 2 characters, and more may have gone out; then native typing took longer than its 3s: niri hadn't acknowledged the key last sent"),
            "{}",
            error.detail
        );
        niri.slow_keys(Duration::ZERO).await;
        marker_cleared(&niri, &dir).await;
        let seen = niri.seen();
        assert!(seen.held_keys().is_empty(), "{:?}", seen.keys);
        assert_eq!(
            seen.last_modifiers().map(|(depressed, _)| depressed),
            Some(0)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A call gets 50 ms a key, here 7.5 s, and a short one wtype's 3 s; past them, the
    /// next key is refused with `deadline_exceeded`.
    #[tokio::test(start_paused = true)]
    async fn a_call_gets_its_keys_time_and_at_least_wtypes() {
        let budget = Budget::start(150);
        let short = Budget::start(2);
        tokio::time::advance(Duration::from_millis(2900)).await;
        short.check().unwrap();
        tokio::time::advance(Duration::from_millis(4500)).await;
        budget.check().unwrap();
        short.check().unwrap_err();
        tokio::time::advance(Duration::from_millis(100)).await;
        let error = budget.check().unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
    }

    fn device() -> Device {
        Device {
            keyboard: None,
            marker: None,
            group: 0,
            socket: Socket::unknown("no niri in this test"),
            aftercare: None,
            ends: None,
        }
    }

    #[tokio::test]
    async fn a_paste_key_its_keeper_doesnt_admit_is_never_committed() {
        let dir = fresh_dir("commit-unadmitted");
        let mut device = device();
        let (aftercare, _done) = fake_aftercare(STOPPED_WAITING).await;
        let mut aftercare = Some(aftercare);
        let desk = desk_read(&dir, Duration::ZERO);
        let error = device.commit(&mut aftercare, &desk).await.unwrap_err();
        assert!(
            error.detail.contains("nothing was pasted"),
            "{}",
            error.detail
        );
        // The stroke after the commit doesn't run, and nothing says the key went out.
        assert!(aftercare.is_none() && device.aftercare.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The keeper's admission may outlast the lock read: the key is then never committed,
    /// and the keeper hears `n` and restores the clipboard.
    #[tokio::test]
    async fn a_paste_key_admitted_after_the_lock_read_aged_is_never_committed() {
        let dir = fresh_dir("commit-aged-lock");
        let mut device = device();
        let (aftercare, done) = fake_aftercare(SLOW_ADMISSION).await;
        let mut aftercare = Some(aftercare);
        // Fresh when the `k` goes out, older than `LAST_GATE` once the keeper admits.
        let desk = desk_read(&dir, Duration::from_millis(200));
        let error = device.commit(&mut aftercare, &desk).await.unwrap_err();
        assert_eq!(error.name, ErrorName::ScreenLocked, "{}", error.detail);
        assert!(aftercare.is_none() && device.aftercare.is_none());
        assert_eq!(
            done.await.unwrap().unwrap(),
            Report::Done {
                read: false,
                clipboard: Clipboard::Restored,
                detail: Some("after n".to_owned())
            }
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
