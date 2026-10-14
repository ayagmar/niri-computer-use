//! `activate_element` and `set_element_text`: an element's own action, or its text
//! replaced, through the accessibility bus instead of the pointer or keyboard. They reach
//! an app without any input event, so they get the keyboard's gates on the element's own
//! window: it must still be the ref's, not be on the deny list, have keyboard focus, and
//! match `expect`. A last gate right before the one call that acts asks all of that
//! again, with the action gate's own checks and the element as it is then, and nothing is
//! retried. That call only says the app took the request (research A4), so the result
//! is what was read back afterwards, and its `effect` whether that showed the action's
//! effect. Names, text and values are never returned or logged.

mod index;

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tokio::time::{Instant, timeout_at};

use crate::a11y::model::{self, NameHash, State, States};
use crate::a11y::{self, A11y, Dispatched, ElementRef, Failed, Request};
use crate::act::{Effect, Observed, Outcome};
use crate::control::desk::LAST_GATE;
use crate::error::{CallError, ErrorName, ToolError};
use crate::input::Input;
use crate::input::keyboard::{self, Expect, Focus};
use crate::niri;
use crate::niri::waiter::{View, Waited, Waiter};
use crate::policy::{self, Loaded};
use niri_ipc::Window;

use super::{Names, gone_is_stale, identify, stale};
use index::{Named, Source};

/// How long an activation's effect has to show before the element is looked at again.
/// GTK 3 and GTK 4 buttons answer the action as they answer Enter: shown pressed for
/// 250 ms (`ACTIVATE_TIMEOUT` in `gtkbutton.c`), then clicked. Qt acts at once. The
/// window closing ends the wait sooner.
const SETTLE: Duration = Duration::from_millis(300);
/// `set_element_text`'s most text, in bytes of UTF-8 (see docs/decisions.md).
pub(crate) const MAX_TEXT: usize = 64 * 1024;

/// What kind an action is, by its name. An app names its actions, and may put anything
/// in a name, so only these are ever logged or said about one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionKind {
    Click,
    Press,
    Activate,
    Toggle,
    /// Any other name.
    Other,
}

impl ActionKind {
    const DEFAULTS: [Self; 4] = [Self::Click, Self::Press, Self::Activate, Self::Toggle];

    /// The kind of the action named `name`, in any case, as Qt names its `Press`.
    fn of(name: &str) -> Self {
        Self::DEFAULTS
            .into_iter()
            .find(|kind| name.eq_ignore_ascii_case(kind.name()))
            .unwrap_or(Self::Other)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Click => "click",
            Self::Press => "press",
            Self::Activate => "activate",
            Self::Toggle => "toggle",
            Self::Other => "other",
        }
    }
}

/// An action of an element: its kind and its index among the element's actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Chosen {
    pub(crate) kind: ActionKind,
    pub(crate) index: usize,
}

/// What an element action did, without the element's name or text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Acted {
    pub(crate) role: &'static str,
    /// `activate_element`: the action taken.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) action: Option<Chosen>,
    /// `activate_element`, observed `present`: the states set since.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) states_set: Vec<&'static str>,
    /// `activate_element`, observed `present`: the states cleared since.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) states_cleared: Vec<&'static str>,
    /// `activate_element`, observed `present`, on an element with a value, read before and
    /// after: whether it changed. Never the value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) value_changed: Option<bool>,
    /// `activate_element`, observed `present`: whether the element's name changed, as a
    /// button that relabels itself does. Never either name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name_changed: Option<bool>,
    /// `set_element_text`: the characters the element says it holds now.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) characters: Option<i32>,
    /// The element's window, for reading niri's change report.
    #[serde(skip)]
    pub(crate) window: u64,
}

impl Acted {
    /// An element of `role` in `window` the app took `action` of, or its text, before
    /// anything was seen.
    fn new(role: u32, window: u64, action: Option<Chosen>) -> Self {
        Self {
            role: model::role_name(role),
            action,
            states_set: Vec::new(),
            states_cleared: Vec::new(),
            value_changed: None,
            name_changed: None,
            characters: None,
            window,
        }
    }
}

/// The index of the action `activate_element` takes among the element's `advertised`
/// ones: `requested`, which must be one of them, or by default the first of them whose
/// kind is click, press, activate or toggle. A mistake says how many actions there are
/// and of which kinds, never their names.
pub(crate) fn choose_action(
    advertised: &[String],
    requested: Option<&str>,
) -> Result<usize, String> {
    let listed = || {
        let mut kinds: Vec<&str> = Vec::new();
        for action in advertised {
            let kind = ActionKind::of(action).name();
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
        format!(
            "the element has {} actions, of the kinds {}; elements lists them",
            advertised.len(),
            kinds.join(", ")
        )
    };
    if let Some(requested) = requested {
        return advertised
            .iter()
            .position(|action| action == requested)
            .ok_or_else(|| format!("`action` isn't one of the element's actions: {}", listed()));
    }
    advertised
        .iter()
        .position(|action| ActionKind::of(action) != ActionKind::Other)
        .ok_or_else(|| {
            format!(
                "none of the element's actions is click, press, activate or toggle, so name one in `action`: {}",
                listed()
            )
        })
}

/// What the audit log keeps of an element action's arguments: the ref in the form refs
/// are issued in, otherwise its length, the role it was listed with, the kind and index of the action it names, the text's length, `expect` and
/// `screenshot`. Never a name, text, or an action's own name.
pub(crate) fn logged(
    id: &str,
    kept: Option<&ElementRef>,
    action: Option<&str>,
    text_len: Option<usize>,
    expect: &Value,
) -> Value {
    let role = kept.map(|element| model::role_name(element.kept.role));
    let id = crate::audit::ref_id(id);
    if let Some(text_len) = text_len {
        return json!({"element": id, "role": role, "text_len": text_len, "expect": expect});
    }
    let index = kept.and_then(|element| choose_action(&element.kept.actions, action).ok());
    let kind = match (kept, index) {
        (Some(element), Some(index)) => element
            .kept
            .actions
            .get(index)
            .map(|name| ActionKind::of(name)),
        _ => action.map(ActionKind::of),
    };
    json!({"element": id, "role": role, "action": kind, "action_index": index, "expect": expect})
}

/// The action gate's checks, which an element action asks again right before its call,
/// since reading the element takes time.
pub(crate) trait Recheck: Sync {
    /// The policy's answer from a fresh readiness report: niri, the policy file and the
    /// lock screen, whose state is read once more after the rest of the report.
    fn control(&self) -> impl Future<Output = Result<(), ToolError>> + Send;
    /// The desk's: the stop flag, the input-dirty marker and the lease.
    fn desk(&self) -> Result<(), ToolError>;
    /// Tells the desk the call that acts is going out, so a stop or a moved lease from now
    /// on leaves the action `uncertain` instead of refused, or refuses it, with nothing
    /// sent, when the lock state was read too long ago.
    fn sending(&self) -> Result<(), ToolError>;
    /// Tells the desk the app took that call, with the outcome as far as it is known, which
    /// a stop or a moved lease from now on reports.
    fn answered(&self, outcome: Outcome);
}

/// What an element action's last gate checks: `expect`, the policy's deny list, and the
/// action gate's checks; and where it notes how long they took.
pub(crate) struct Gate<'a, R> {
    pub(crate) expect: Expect,
    pub(crate) policy: &'a Loaded,
    pub(crate) recheck: &'a R,
    pub(crate) times: &'a GateTimes,
}

/// How long each pass of an element action's last gate took and how old the answers it
/// relied on were when the call went out, for the audit log, which shows from them how
/// much of `LAST_GATE` the checks use.
#[derive(Debug, Default)]
pub(crate) struct GateTimes(Mutex<Timed>);

/// The last gate's times, in milliseconds.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
struct Timed {
    /// Each pass, in order: how long it took, or `LAST_GATE` when the clock cut it short.
    passes_ms: Vec<f64>,
    /// How old the answers were when the call went out, or would have gone out.
    #[serde(skip_serializing_if = "Option::is_none")]
    sent_ms: Option<f64>,
}

impl GateTimes {
    fn pass(&self, took: Duration) {
        self.timed().passes_ms.push(millis(took));
    }

    fn sent(&self, age: Duration) {
        self.timed().sent_ms = Some(millis(age));
    }

    fn timed(&self) -> std::sync::MutexGuard<'_, Timed> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The audit log's arguments `logged`, with the times as `gate` once a pass ran.
    pub(crate) fn logged(&self, mut logged: Value) -> Value {
        let timed = self.timed().clone();
        if let (false, Some(fields)) = (timed.passes_ms.is_empty(), logged.as_object_mut()) {
            fields.insert("gate".to_owned(), json!(timed));
        }
        logged
    }
}

/// `took` in milliseconds, to a tenth of one.
fn millis(took: Duration) -> f64 {
    (took.as_secs_f64() * 10_000.0).round() / 10.0
}

/// What the last gate saw, with what the tool's own read of the element found, and when
/// the pass that read the answers it relies on began.
struct Checked<T> {
    read: T,
    role: u32,
    states: States,
    focus: Focus,
    began: Instant,
}

/// What a pass of the last gate saw: the tool's own read, the element's role and states,
/// and the focus.
type Passed<T> = (T, u32, States, Focus);

impl<R: Recheck> Gate<'_, R> {
    /// The last checks before the call that acts, `pass`, done within `LAST_GATE` of
    /// their first read; `send` checks no answer they rely on is older when the call goes
    /// out. A pass that runs over is done once more from the start; a second one that runs
    /// over is `element_busy`. A change inside that window can still be missed.
    async fn last<T, F>(
        &self,
        element: &ElementRef,
        request: &Request,
        waiter: &mut Waiter,
        read: impl Fn() -> F,
    ) -> Result<Checked<T>, CallError>
    where
        F: Future<Output = Result<T, CallError>>,
    {
        for _ in 0..2 {
            let began = Instant::now();
            let pass = self.pass(element, request, waiter, read());
            let passed = timeout_at(began + LAST_GATE, pass).await;
            self.times.pass(began.elapsed());
            if let Ok(passed) = passed {
                let (found, role, states, focus) = passed?;
                return Ok(Checked {
                    read: found,
                    role,
                    states,
                    focus,
                    began,
                });
            }
        }
        let took = format!("took over {} ms twice", LAST_GATE.as_millis());
        Err(busy(&took).into())
    }

    /// Tells the desk the call that acts goes out, if the answers the last gate relied on
    /// are still at most `LAST_GATE` old: this is the moment before it goes out.
    fn send<T>(&self, checked: &Checked<T>) -> Result<(), CallError> {
        let age = checked.began.elapsed();
        self.times.sent(age);
        if age > LAST_GATE {
            let took = format!("ended over {} ms after they began", LAST_GATE.as_millis());
            return Err(busy(&took).into());
        }
        self.recheck.sending()?;
        Ok(())
    }

    /// One pass of the last checks. The app's answers come first, one call at a time: the
    /// element is still the one listed, then `read`, the tool's own read of it, then its
    /// states, then its role, so a field that became a password field meanwhile is
    /// refused. Then ours, none of which waits on the app: the action gate's checks again
    /// with the lock state read last, niri's events received so far, failing if the
    /// stream was lost, the window's focus, deny list and `expect` on them, and the desk
    /// last. niri, Noctalia, the desk and the app are asked apart, and nothing makes those
    /// reads one step: `last` bounds how old they are.
    async fn pass<T>(
        &self,
        element: &ElementRef,
        request: &Request,
        waiter: &mut Waiter,
        read: impl Future<Output = Result<T, CallError>>,
    ) -> Result<Passed<T>, CallError> {
        let view = waiter.view();
        identify(
            request,
            element,
            owner(view, element)?,
            view.windows().values(),
            Names::All,
        )
        .await?;
        let read = read.await?;
        let (role, states) = current(request, element).await?;
        self.recheck.control().await?;
        if let Some(why) = caught_up(waiter).await {
            return Err(ToolError::new(
                ErrorName::NiriUnavailable,
                format!("{why}; nothing was sent"),
            )
            .into());
        }
        let focus = owner_focused(self.policy, waiter.view(), element, &self.expect)?;
        self.recheck.desk()?;
        Ok((read, role, states, focus))
    }
}

/// `element_busy`: the last gate's checks `took` too long.
fn busy(took: &str) -> ToolError {
    ToolError::new(
        ErrorName::ElementBusy,
        format!(
            "the app or the desktop answered too slowly to check the element: its last checks {took}; nothing was sent"
        ),
    )
}

/// Takes the element's action `requested`, or its default one, and looks at the element
/// a moment later, its states and value against those the last gate read.
pub(crate) async fn activate(
    input: Input<'_>,
    element: &ElementRef,
    requested: Option<&str>,
    gate: Gate<'_, impl Recheck>,
) -> Result<Outcome, CallError> {
    let a11y = accessible(input)?;
    let index =
        choose_action(&element.kept.actions, requested).map_err(CallError::InvalidArguments)?;
    let action = Chosen {
        kind: element
            .kept
            .actions
            .get(index)
            .map_or(ActionKind::Other, |name| ActionKind::of(name)),
        index,
    };
    let mut waiter = niri::waiter(input.niri.events).await?;
    owner_focused(gate.policy, waiter.view(), element, &gate.expect)?;
    let request = a11y.request(a11y::BUDGET).await?.names_only();
    // The fast refusals, before the last gate asks all of it again.
    identify(
        &request,
        element,
        owner(waiter.view(), element)?,
        waiter.view().windows().values(),
        Names::All,
    )
    .await?;
    let mapped = canonical(&request, element, action).await?;
    let checked = gate
        .last(element, &request, &mut waiter, || async {
            let at = offered(&request, element, action, &mapped).await?;
            Ok((at, value(&request, element).await))
        })
        .await?;
    gate.send(&checked)?;
    let (at, value) = checked.read;
    // The last gate found the element's name to be the one it was listed with.
    let before = Before {
        states: checked.states,
        value,
        name: element.kept.lineage.last().map(|link| link.name),
    };
    let dispatched = request
        .do_action(element, at)
        .await
        .map_err(gone_is_stale)?;
    let lost = taken(dispatched, &format!("its {} action", action.kind.name()))?;
    let window = element.kept.window;
    let acted = Acted::new(checked.role, window, Some(action));
    if lost.is_none() {
        gate.recheck
            .answered(answered(checked.focus, acted.clone()));
    }
    let (after, stream) = settled(&mut waiter, &request, element, &before).await;
    let doubt = Doubt {
        reply: lost,
        stream,
    };
    let (seen, looked) = activation(before, after);
    let acted = Acted {
        states_set: looked.states_set,
        states_cleared: looked.states_cleared,
        value_changed: looked.value_changed,
        name_changed: looked.name_changed,
        ..acted
    };
    Ok(done(seen, doubt, waiter.view(), checked.focus, acted))
}

/// The element's value as read: `None` when it has no `Value` interface, or why it
/// couldn't be read.
type Reading = Result<Option<f64>, String>;

/// Reads the element's value. A failure is kept as why, since it never stops the action.
async fn value(request: &Request, element: &ElementRef) -> Reading {
    request
        .current_value(element)
        .await
        .map_err(|failed| ToolError::from(failed).detail)
}

/// Waits `SETTLE` for the activation's effect, ended sooner by the element's window closing,
/// then reads the element again unless it did; and why niri's event stream was lost, if it
/// was.
async fn settled(
    waiter: &mut Waiter,
    request: &Request,
    element: &ElementRef,
    before: &Before,
) -> (After, Option<String>) {
    let window = element.kept.window;
    let (gone, stream) = match waiter
        .until(SETTLE, |view| {
            (!view.windows().contains_key(&window)).then_some(())
        })
        .await
    {
        Waited::Done(()) => (true, None),
        Waited::Timeout => (false, None),
        Waited::Lost(why) => (false, Some(why)),
    };
    let after = if gone {
        After::WindowGone
    } else {
        probe(request, element, before).await
    };
    let stream = match stream {
        Some(why) => Some(why),
        None => caught_up(waiter).await,
    };
    (after, stream)
}

/// What the last gate read of an activated element, which the look afterwards is compared
/// with: its states, its value, and its name as listed, as a hash.
#[derive(Debug)]
struct Before {
    states: States,
    value: Reading,
    name: Option<NameHash>,
}

/// The element's name as a hash, which is all that is kept of it, or why it couldn't be
/// read.
type Label = Result<NameHash, String>;

/// The activated element's role and states now, then its value when it had one `before`,
/// then its name.
async fn probe(request: &Request, element: &ElementRef, before: &Before) -> After {
    let states = match request.state(element).await {
        Ok((_, states)) => states,
        Err(failed) => return After::Probed(Err(failed)),
    };
    let value = match before.value {
        Ok(Some(_)) => value(request, element).await,
        _ => Ok(None),
    };
    let name = request
        .name(element.at())
        .await
        .map(|name| NameHash::of(&name))
        .map_err(|failed| ToolError::from(failed).detail);
    After::Probed(Ok(Now {
        states,
        value,
        name,
    }))
}

/// The most action indices `canonical` asks about.
const MAX_ACTIONS: i32 = 32;

/// An action's `DoAction` index and the name that showed it.
#[derive(PartialEq, Eq)]
struct Mapped {
    index: i32,
    source: Source,
    named: Named,
}

/// The index to ask the app for `action`, its `DoAction` index (see `index`): the element
/// must still list the action where `elements` did, and the app names each of its actions
/// from 0 until it has no more, at most 32. With no index naming it, several, or any
/// whose name is unknown, which index the app would run is unknown, so nothing is sent.
/// Actions that changed meanwhile can look like that, so the element must then still list
/// the action as before, or it is stale.
async fn canonical(
    request: &Request,
    element: &ElementRef,
    action: Chosen,
) -> Result<Mapped, CallError> {
    let place = still_listed(request, element, action).await?;
    if let Some(mapped) = search(request, element, &place).await? {
        return Ok(mapped);
    }
    still_listed(request, element, action).await?;
    Err(unmapped_action(action))
}

/// The one index whose action has `place`'s name, asking the app for its actions' names
/// from 0 until it has no more, at most 32.
async fn search(
    request: &Request,
    element: &ElementRef,
    place: &Place<'_>,
) -> Result<Option<Mapped>, CallError> {
    let mut table = Vec::new();
    for at in 0..MAX_ACTIONS {
        let answers = request
            .action_names(element, at)
            .await
            .map_err(gone_is_stale)?;
        if index::past_end(table.len(), place.count, &answers) {
            let found = index::find(&table, place.source, place.name);
            return Ok(found.and_then(|(index, named)| {
                Some(Mapped {
                    index: i32::try_from(index).ok()?,
                    source: place.source,
                    named,
                })
            }));
        }
        table.push(answers);
    }
    Ok(None)
}

/// The index `canonical` found for `action`, found again the same way: the element still
/// lists the action where `elements` did, so an action list that changed since can't make
/// the index name another action, and the search gives the same index, source and name,
/// so no other index has taken the name since.
async fn offered(
    request: &Request,
    element: &ElementRef,
    action: Chosen,
    mapped: &Mapped,
) -> Result<i32, CallError> {
    let place = still_listed(request, element, action).await?;
    if search(request, element, &place).await?.as_ref() == Some(mapped) {
        return Ok(mapped.index);
    }
    Err(stale(&format!(
        "the app no longer names the element's {} action at its index",
        action.kind.name()
    ))
    .into())
}

/// Where an element lists an action now.
struct Place<'a> {
    /// The name `elements` listed it by.
    name: &'a str,
    source: Source,
    /// How many actions the element lists.
    count: usize,
}

/// Where the element lists `action` now: at its index, with the name `elements` listed
/// there.
async fn still_listed<'a>(
    request: &Request,
    element: &'a ElementRef,
    action: Chosen,
) -> Result<Place<'a>, CallError> {
    let listed = request.listed(element).await.map_err(gone_is_stale)?;
    let count = listed.len();
    let source = match listed.get(action.index) {
        Some(name) if name.is_empty() => Source::Own,
        _ => Source::List,
    };
    let actions = request.named(element.at(), listed).await?;
    match element.kept.actions.get(action.index) {
        Some(name) if actions.get(action.index) == Some(name) => Ok(Place {
            name,
            source,
            count,
        }),
        _ => Err(stale(&format!(
            "the element no longer offers its {} action at index {}",
            action.kind.name(),
            action.index
        ))
        .into()),
    }
}

/// `element_unmappable` for an action `canonical` couldn't find the index of.
fn unmapped_action(action: Chosen) -> CallError {
    ToolError::new(
        ErrorName::ElementUnmappable,
        format!(
            "action_unmapped: not exactly one of the app's actions is known to have the name the element lists its {} action by, so which action the app would run isn't known; nothing was sent, so aim at screenshot pixels instead",
            action.kind.name()
        ),
    )
    .into()
}

/// Replaces the element's whole text with `text` and reads it back.
pub(crate) async fn set_text(
    input: Input<'_>,
    element: &ElementRef,
    text: &str,
    gate: Gate<'_, impl Recheck>,
) -> Result<Outcome, CallError> {
    check_text(text)?;
    let a11y = accessible(input)?;
    let mut waiter = niri::waiter(input.niri.events).await?;
    owner_focused(gate.policy, waiter.view(), element, &gate.expect)?;
    let request = a11y.request(a11y::BUDGET).await?.names_only();
    // The fast refusals, before the last gate asks all of it again.
    identify(
        &request,
        element,
        owner(waiter.view(), element)?,
        waiter.view().windows().values(),
        Names::All,
    )
    .await?;
    editable(&request, element).await?;
    let checked = gate
        .last(element, &request, &mut waiter, || {
            editable(&request, element)
        })
        .await?;
    if !checked.states.has(State::Editable) {
        return Err(CallError::InvalidArguments(
            "the element isn't in the `editable` state, so its text can't be set".to_owned(),
        ));
    }
    gate.send(&checked)?;
    let dispatched = request
        .set_text(element, text)
        .await
        .map_err(gone_is_stale)?;
    let lost = taken(dispatched, "the new text")?;
    let acted = Acted::new(checked.role, element.kept.window, None);
    if lost.is_none() {
        gate.recheck
            .answered(answered(checked.focus, acted.clone()));
    }
    let held = read_back(&request, element, text).await;
    let doubt = Doubt {
        reply: lost,
        stream: caught_up(&mut waiter).await,
    };
    let (seen, characters) = written(held);
    let acted = Acted {
        characters,
        ..acted
    };
    Ok(done(seen, doubt, waiter.view(), checked.focus, acted))
}

/// What a field holds after its text was set, by how many characters, compared with the
/// text sent. Never the text.
#[derive(Debug, PartialEq, Eq)]
enum Held {
    /// The text sent.
    Same(i32),
    /// Other text.
    Other(i32),
}

/// Reads the field's text back and compares it with `sent`: its character count first,
/// and only when that is the text's, at most `MAX_TEXT` characters, the text itself, which
/// is dropped once compared.
async fn read_back(request: &Request, element: &ElementRef, sent: &str) -> Result<Held, Failed> {
    let count = request.character_count(element).await?;
    if usize::try_from(count) != Ok(sent.chars().count()) {
        return Ok(Held::Other(count));
    }
    let text = request.text(element, count).await?;
    Ok(if text == sent {
        Held::Same(count)
    } else {
        Held::Other(count)
    })
}

/// Applies niri's events received so far to the waiter's view, so its focus is the one now;
/// why not, when the stream was lost.
async fn caught_up(waiter: &mut Waiter) -> Option<String> {
    match waiter.until(Duration::ZERO, |_| None::<()>).await {
        Waited::Lost(why) => Some(why),
        Waited::Done(()) | Waited::Timeout => None,
    }
}

/// Checks the element has an `EditableText` interface, or its text can't be set.
async fn editable(request: &Request, element: &ElementRef) -> Result<(), CallError> {
    if request
        .editable_text(element)
        .await
        .map_err(gone_is_stale)?
    {
        return Ok(());
    }
    Err(CallError::InvalidArguments(
        "the element has no EditableText interface, so its text can't be set".to_owned(),
    ))
}

/// Refuses text over `MAX_TEXT` bytes before anything is asked.
fn check_text(text: &str) -> Result<(), CallError> {
    if text.len() > MAX_TEXT {
        return Err(ToolError::new(
            ErrorName::TextTooLong,
            format!(
                "{} bytes; at most {MAX_TEXT} bytes of UTF-8 per call; nothing was set",
                text.len()
            ),
        )
        .into());
    }
    Ok(())
}

fn accessible(input: Input<'_>) -> Result<&A11y, ToolError> {
    input
        .a11y
        .ok_or_else(|| a11y::not_accessible("this session has no accessibility bus".to_owned()))
}

/// Checks the element's window in niri's `view`: still the one the ref was listed in, not
/// denied, and the window with keyboard focus, which `expect` must name as well. A shell
/// panel or a lock screen holds keyboard focus with no window focused, so an element
/// under it is refused here.
fn owner_focused(
    policy: &Loaded,
    view: &View,
    element: &ElementRef,
    expect: &Expect,
) -> Result<Focus, CallError> {
    if *expect == Expect::Unchecked {
        return Err(CallError::InvalidArguments(
            "element actions always check focus: give `expect` as {\"window_id\"} or {\"app_id\"}, not \"none\"".to_owned(),
        ));
    }
    let window = owner(view, element)?;
    if let Some(refused) = policy::refuse_window(policy, window.id, window.app_id.as_deref()) {
        return Err(refused.into());
    }
    if view.focused_window() != Some(window.id) {
        let focused = view.focused_window().map_or_else(
            || "no window has keyboard focus".to_owned(),
            |id| format!("window {id} has it"),
        );
        return Err(ToolError::new(
            ErrorName::FocusMismatch,
            format!(
                "the element's window {} doesn't have keyboard focus; {focused}",
                window.id
            ),
        )
        .into());
    }
    Ok(keyboard::check_expect(expect, view)?)
}

/// The element's window in niri's `view`, if it is still the one the ref was listed in.
fn owner<'a>(view: &'a View, element: &ElementRef) -> Result<&'a Window, ToolError> {
    let kept = &element.kept;
    view.windows()
        .get(&kept.window)
        .filter(|window| window.pid == Some(kept.pid))
        .ok_or_else(|| stale(&format!("window {} is gone", kept.window)))
}

/// The element's role and states now, if it is still the element listed, not a password
/// field, and showing. The role is read last.
async fn current(request: &Request, element: &ElementRef) -> Result<(u32, States), ToolError> {
    let (role, states) = request.state(element).await.map_err(gone_is_stale)?;
    // Before the role is compared, so a field that became a password field says so.
    if let Some(refused) = policy::refuse_secret_field(model::role_name(role)) {
        return Err(refused);
    }
    if role != element.kept.role {
        return Err(stale(&format!(
            "the element was a {} and is now a {}",
            model::role_name(element.kept.role),
            model::role_name(role)
        )));
    }
    if !(states.has(State::Showing) && states.has(State::Visible)) {
        return Err(super::unmappable(model::Unmappable::NotShowing));
    }
    Ok((role, states))
}

/// What became of an activated element after the settle.
#[derive(Debug)]
enum After {
    /// niri closed its window.
    WindowGone,
    /// It as read again, or why its states couldn't be read.
    Probed(Result<Now, Failed>),
}

/// An activated element as read after the settle.
#[derive(Debug)]
struct Now {
    states: States,
    value: Reading,
    name: Label,
}

/// What an activation changed in an element still present.
#[derive(Debug, Default, PartialEq)]
struct Looked {
    states_set: Vec<&'static str>,
    states_cleared: Vec<&'static str>,
    value_changed: Option<bool>,
    name_changed: Option<bool>,
}

/// `gone` when the window or the element went away, or the element says it is `defunct`,
/// as Chromium's does once it replaced the element; `present` with what changed since
/// `before`; or `unknown` with why the element couldn't be read.
fn activation(before: Before, after: After) -> (Seen, Looked) {
    let gone = Seen::new(Observed::Gone, Effect::Seen, None);
    match after {
        After::WindowGone | After::Probed(Err(Failed::Gone(_))) => (gone, Looked::default()),
        After::Probed(Ok(now)) if now.states.has(State::Defunct) => (gone, Looked::default()),
        After::Probed(Ok(now)) => present(before, now),
        After::Probed(Err(Failed::Refused(error) | Failed::Error(error))) => {
            let unknown = Seen::new(Observed::Unknown, Effect::Unverified, Some(error.detail));
            (unknown, Looked::default())
        }
    }
}

/// An element still present: its states, value and name compared with `before`. The effect
/// is `seen` when any of them changed, `none_seen` when none did, and `unverified` when its
/// value or name couldn't be compared and nothing else changed.
fn present(before: Before, now: Now) -> (Seen, Looked) {
    let (states_set, states_cleared) = now.states.changes(before.states);
    let value = compare(before.value, now.value);
    let name = renamed(before.name, now.name);
    let value_changed = value.as_ref().ok().copied().flatten();
    let name_changed = name.as_ref().ok().copied().flatten();
    let changed = !(states_set.is_empty() && states_cleared.is_empty())
        || value_changed == Some(true)
        || name_changed == Some(true);
    let seen = match (changed, value.and(name)) {
        (true, _) => Seen::new(Observed::Present, Effect::Seen, None),
        (false, Ok(_)) => Seen::new(Observed::Present, Effect::NoneSeen, None),
        (false, Err(why)) => Seen::new(Observed::Present, Effect::Unverified, Some(why)),
    };
    let looked = Looked {
        states_set,
        states_cleared,
        value_changed,
        name_changed,
    };
    (seen, looked)
}

/// Whether the element's name changed since it was listed: `None` with no name to compare
/// with, or why the name couldn't be read again.
fn renamed(before: Option<NameHash>, after: Label) -> Result<Option<bool>, String> {
    let Some(before) = before else {
        return Ok(None);
    };
    after.map(|after| Some(after != before))
}

/// Whether an element's value changed between two readings: `None` when it has none, or
/// why the two can't be compared.
fn compare(before: Reading, after: Reading) -> Result<Option<bool>, String> {
    match (before?, after?) {
        (None, _) => Ok(None),
        (Some(before), Some(after)) => Ok(Some(before.total_cmp(&after).is_ne())),
        (Some(_), None) => Err("the element's value couldn't be read again".to_owned()),
    }
}

/// `matched` when the field holds the text that was set, `differs` with its character
/// count when it holds other text, as when an app filters, caps or reformats input, or
/// `unknown` with why it couldn't be read back.
fn written(held: Result<Held, Failed>) -> (Seen, Option<i32>) {
    match held {
        Ok(Held::Same(count)) => (
            Seen::new(Observed::Matched, Effect::Seen, None),
            Some(count),
        ),
        Ok(Held::Other(count)) => (
            Seen::new(Observed::Differs, Effect::Differs, None),
            Some(count),
        ),
        Err(failed) => {
            let why = Some(ToolError::from(failed).detail);
            (Seen::new(Observed::Unknown, Effect::Unverified, why), None)
        }
    }
}

/// What was read back after the call: what was observed, whether it showed the effect,
/// and why not when it couldn't.
#[derive(Debug, PartialEq, Eq)]
struct Seen {
    observed: Observed,
    effect: Effect,
    detail: Option<String>,
}

impl Seen {
    const fn new(observed: Observed, effect: Effect, detail: Option<String>) -> Self {
        Self {
            observed,
            effect,
            detail,
        }
    }
}

/// Why an element action's outcome is `uncertain`, if it is.
#[derive(Debug)]
struct Doubt {
    /// The call's reply was lost, so whether the app took it is unknown.
    reply: Option<String>,
    /// niri's event stream was lost after the call, so focus afterwards is unknown.
    stream: Option<String>,
}

/// The outcome: what was `seen` afterwards, or, in `doubt`, `uncertain`, saying what was
/// seen in `detail`: with `accepted: null` when the reply was lost, and with no focused
/// window when niri's events were.
fn done(seen: Seen, doubt: Doubt, view: &View, focus: Focus, acted: Acted) -> Outcome {
    let Seen {
        observed,
        effect,
        detail,
    } = seen;
    if doubt.reply.is_none() && doubt.stream.is_none() {
        return Outcome {
            focus: Some(focus),
            element: Some(acted),
            effect: Some(effect),
            detail,
            ..Outcome::seen(observed, view, Vec::new())
        };
    }
    let accepted = doubt.reply.is_none().then_some(true);
    let view = doubt.stream.is_none().then_some(view);
    let lost = [doubt.reply, doubt.stream]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ");
    let afterwards = serde_json::to_value(observed)
        .ok()
        .and_then(|name| name.as_str().map(str::to_owned))
        .unwrap_or_default();
    let why = detail.map_or_else(String::new, |detail| format!(" ({detail})"));
    Outcome {
        focus: Some(focus),
        element: Some(acted),
        effect: Some(effect),
        ..Outcome::uncertain(
            accepted,
            view,
            format!("{lost}; afterwards: {afterwards}{why}"),
        )
    }
}

/// The outcome as far as it is known once the app took the call: accepted, and the look
/// afterwards still to come, so its effect `unverified`. A stop or a moved lease from then
/// on reports it, with why.
fn answered(focus: Focus, acted: Acted) -> Outcome {
    Outcome {
        focus: Some(focus),
        element: Some(acted),
        effect: Some(Effect::Unverified),
        ..Outcome::uncertain(Some(true), None, String::new())
    }
}

/// Whether the app took the request: `None` when it answered that it did, why its reply
/// was lost when that is unknown, and `upstream_error` when it refused `what`.
fn taken(dispatched: Dispatched<bool>, what: &str) -> Result<Option<String>, ToolError> {
    match dispatched {
        Dispatched::Answered(true) => Ok(None),
        Dispatched::Answered(false) => Err(refused_by_app(what)),
        Dispatched::Lost(why) => Ok(Some(why)),
    }
}

fn refused_by_app(what: &str) -> ToolError {
    ToolError::new(
        ErrorName::UpstreamError,
        format!("the app refused {what}; it may have been taken, so take a screenshot to check"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::a11y::model::{Kept, Row};
    use crate::niri::waiter::tests::{view, window};
    use crate::policy::parse;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|&name| name.to_owned()).collect()
    }

    fn element(window: u64, actions: &[&str]) -> ElementRef {
        ElementRef {
            bus: ":1.7".to_owned(),
            path: "/org/a11y/atspi/accessible/9".to_owned(),
            frame: "/org/a11y/atspi/accessible/1".to_owned(),
            kept: Kept {
                role: 43,
                window,
                pid: 1,
                actions: names(actions),
                lineage: Vec::new(),
                row: Row::Lineage,
            },
        }
    }

    /// The action gate's checks, all passing, counting the calls that went out.
    #[derive(Default)]
    struct Sent(std::sync::atomic::AtomicUsize);

    impl Recheck for Sent {
        fn control(&self) -> impl Future<Output = Result<(), ToolError>> + Send {
            std::future::ready(Ok(()))
        }

        fn desk(&self) -> Result<(), ToolError> {
            Ok(())
        }

        fn sending(&self) -> Result<(), ToolError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }

        fn answered(&self, _: Outcome) {}
    }

    #[test]
    fn a_call_goes_out_only_while_the_last_gates_answers_are_young_enough() {
        let policy = Loaded::Valid(parse("", false).unwrap());
        let sent = Sent::default();
        let times = GateTimes::default();
        let gate = Gate {
            expect: Expect::Window(3),
            policy: &policy,
            recheck: &sent,
            times: &times,
        };
        let checked = |began| Checked {
            read: (),
            role: 43,
            states: States::from_words(&[]),
            focus: Focus::Matched,
            began,
        };
        let now = Instant::now();
        assert!(gate.send(&checked(now)).is_ok());
        let late = now
            .checked_sub(LAST_GATE + Duration::from_millis(1))
            .unwrap();
        let Err(CallError::Tool(refused)) = gate.send(&checked(late)) else {
            panic!("a late call went out");
        };
        // The audit log gets the age of the answers the refused call would have relied on.
        times.pass(Duration::from_millis(12));
        let logged = times.logged(json!({"element": "e"}));
        assert!(
            logged["gate"]["sent_ms"].as_f64().unwrap() >= 251.0,
            "{logged}"
        );
        assert_eq!(refused.name, ErrorName::ElementBusy);
        assert!(
            refused.detail.ends_with("nothing was sent"),
            "{}",
            refused.detail
        );
        assert_eq!(sent.0.into_inner(), 1);
    }

    #[test]
    fn the_default_action_is_the_elements_first_click_press_activate_or_toggle() {
        let gtk = names(&["click"]);
        assert_eq!(choose_action(&gtk, None), Ok(0));
        // Qt Quick's button: Press, then SetFocus.
        let qt = names(&["SetFocus", "Press"]);
        assert_eq!(choose_action(&qt, None), Ok(1));
        // The element's own order decides, not the list's.
        let entry = names(&["activate", "click"]);
        assert_eq!(choose_action(&entry, None), Ok(0));
        assert!(choose_action(&[], None).is_err());
    }

    #[test]
    fn a_requested_action_must_be_one_the_element_listed_exactly() {
        let frame = names(&["window.close", "default.activate"]);
        assert_eq!(choose_action(&frame, Some("default.activate")), Ok(1));
        assert!(choose_action(&frame, Some("Window.Close")).is_err());
    }

    #[test]
    fn a_mistake_counts_the_actions_and_names_their_kinds_never_the_apps_names() {
        let label = names(&["SECRET-1 copy", "SECRET-2", "Toggle"]);
        for requested in [None, Some("SECRET-3")] {
            let mistake = choose_action(&label[..2], requested).unwrap_err();
            assert!(!mistake.contains("SECRET"), "{mistake}");
            assert!(
                mistake.contains("2 actions, of the kinds other;"),
                "{mistake}"
            );
        }
        let mistake = choose_action(&label, Some("SECRET-3")).unwrap_err();
        assert!(
            mistake.contains("3 actions, of the kinds other, toggle;"),
            "{mistake}"
        );
    }

    #[test]
    fn only_the_focused_window_that_listed_the_element_and_expect_names_is_acted_on() {
        let policy = Loaded::Valid(parse("deny_input_app_ids = [\"secret\"]", false).unwrap());
        let windows = |focused: u64| {
            view(vec![
                window(3, Some("app"), 1, focused == 3),
                window(4, Some("secret"), 1, focused == 4),
                window(5, Some("other"), 1, focused == 5),
                window(6, Some("app"), 1, focused == 6),
            ])
        };
        let check = |focused: u64, element: &ElementRef, expect: Expect| {
            owner_focused(&policy, &windows(focused), element, &expect)
        };
        let error = |result: Result<Focus, CallError>| match result {
            Err(CallError::Tool(error)) => error.name,
            other => panic!("{other:?}"),
        };
        let mine = element(3, &[]);
        assert_eq!(check(3, &mine, Expect::Window(3)).unwrap(), Focus::Matched);
        assert_eq!(
            check(3, &mine, Expect::App("app".to_owned())).unwrap(),
            Focus::Matched
        );
        // Another window has focus, or none does, as under a shell panel.
        assert_eq!(
            error(check(5, &mine, Expect::Window(3))),
            ErrorName::FocusMismatch
        );
        assert_eq!(
            error(check(0, &mine, Expect::Window(3))),
            ErrorName::FocusMismatch
        );
        // Another window of the element's app has focus: `expect` naming the app matches it,
        // but the element's own window must be the focused one.
        assert_eq!(
            error(check(6, &mine, Expect::App("app".to_owned()))),
            ErrorName::FocusMismatch
        );
        // The element's window has focus, but `expect` names another.
        assert_eq!(
            error(check(3, &mine, Expect::App("other".to_owned()))),
            ErrorName::FocusMismatch
        );
        assert!(matches!(
            check(3, &mine, Expect::Unchecked),
            Err(CallError::InvalidArguments(_))
        ));
        // A denied window is refused even with focus.
        assert_eq!(
            error(check(4, &element(4, &[]), Expect::Window(4))),
            ErrorName::AppDenied
        );
        // The window closed, or its id now belongs to another process.
        assert_eq!(
            error(check(3, &element(9, &[]), Expect::Window(9))),
            ErrorName::ElementStale
        );
        let reused = ElementRef {
            kept: Kept {
                pid: 2,
                ..mine.kept.clone()
            },
            ..mine
        };
        assert_eq!(
            error(check(3, &reused, Expect::Window(3))),
            ErrorName::ElementStale
        );
    }

    /// What an activation with no value saw: observed and effect, and the states changed.
    /// What the last gate read of a button named `Save` with `states` and `value`.
    fn before(states: States, value: Reading) -> Before {
        Before {
            states,
            value,
            name: Some(NameHash::of("Save")),
        }
    }

    /// The element read again with `states`, `value` and `name`.
    fn now(states: States, value: Reading, name: &str) -> After {
        After::Probed(Ok(Now {
            states,
            value,
            name: Ok(NameHash::of(name)),
        }))
    }

    /// What an activation of `Save` with no value saw: observed and effect, and what changed.
    fn activated(states: States, after: After) -> (Observed, Effect, Looked) {
        let (seen, looked) = activation(before(states, Ok(None)), after);
        (seen.observed, seen.effect, looked)
    }

    /// Nothing changed, the name compared.
    const UNCHANGED: Looked = Looked {
        states_set: Vec::new(),
        states_cleared: Vec::new(),
        value_changed: None,
        name_changed: Some(false),
    };

    #[test]
    fn an_activation_is_gone_present_with_its_state_changes_or_unknown() {
        let unchecked = States::from_words(&[(1 << 25) | (1 << 30)]);
        let checked = States::from_words(&[(1 << 4) | (1 << 25) | (1 << 30)]);
        let gone = (Observed::Gone, Effect::Seen, Looked::default());
        assert_eq!(activated(unchecked, After::WindowGone), gone);
        let left = After::Probed(Err(Failed::Gone("gone".to_owned())));
        assert_eq!(activated(unchecked, left), gone);
        let toggled = Looked {
            states_set: vec!["checked"],
            ..UNCHANGED
        };
        assert_eq!(
            activated(unchecked, now(checked, Ok(None), "Save")),
            (Observed::Present, Effect::Seen, toggled)
        );
        // A plain button whose states and name don't change: nothing to see, which isn't a
        // failure.
        assert_eq!(
            activated(checked, now(checked, Ok(None), "Save")),
            (Observed::Present, Effect::NoneSeen, UNCHANGED)
        );
        // Chromium keeps a replaced element on the bus with only `defunct` set.
        let defunct = States::from_words(&[1 << 6]);
        assert_eq!(activated(unchecked, now(defunct, Ok(None), "")), gone);
        let late = ToolError::new(ErrorName::DeadlineExceeded, "no answer within 1 s");
        let (seen, _) = activation(
            before(unchecked, Ok(None)),
            After::Probed(Err(Failed::Error(late))),
        );
        assert_eq!(
            seen,
            Seen::new(
                Observed::Unknown,
                Effect::Unverified,
                Some("no answer within 1 s".to_owned())
            )
        );
    }

    // A counter or play/pause button relabels itself and changes no state: compared by
    // states alone, that read as nothing seen.
    #[test]
    fn a_button_that_relabelled_itself_is_seen() {
        let showing = States::from_words(&[(1 << 25) | (1 << 30)]);
        let relabelled = Looked {
            name_changed: Some(true),
            ..UNCHANGED
        };
        assert_eq!(
            activated(showing, now(showing, Ok(None), "Saved")),
            (Observed::Present, Effect::Seen, relabelled)
        );
        // A name that couldn't be read again can't show there was no effect.
        let unread = After::Probed(Ok(Now {
            states: showing,
            value: Ok(None),
            name: Err("Properties.Get: no answer within 1 s".to_owned()),
        }));
        let (seen, looked) = activation(before(showing, Ok(None)), unread);
        assert_eq!(seen.effect, Effect::Unverified);
        assert!(seen.detail.unwrap().contains("no answer"));
        assert_eq!(looked.name_changed, None);
    }

    // A slider or spin button stepped by its action changes its value and none of its
    // states: without the value, that read as nothing seen.
    #[test]
    fn a_value_that_changed_with_no_state_change_is_seen() {
        let showing = States::from_words(&[(1 << 25) | (1 << 30)]);
        let stepped =
            |was: Reading, is: Reading| activation(before(showing, was), now(showing, is, "Save"));
        let (up, up_looked) = stepped(Ok(Some(2.0)), Ok(Some(3.0)));
        assert_eq!(up.effect, Effect::Seen);
        assert_eq!(up.observed, Observed::Present);
        assert_eq!(up_looked.value_changed, Some(true));
        let (still, still_looked) = stepped(Ok(Some(3.0)), Ok(Some(3.0)));
        assert_eq!(still.effect, Effect::NoneSeen);
        assert_eq!(still_looked.value_changed, Some(false));
    }

    // A value that couldn't be read, before or after, leaves the effect unverified rather
    // than `none_seen`, with why; states that changed still show the effect.
    #[test]
    fn a_value_that_couldnt_be_compared_is_unverified_unless_states_changed() {
        let showing = States::from_words(&[(1 << 25) | (1 << 30)]);
        let checked = States::from_words(&[(1 << 4) | (1 << 25) | (1 << 30)]);
        let failed = || Err("Value.CurrentValue: no answer within 1 s".to_owned());
        for (was, is) in [(failed(), Ok(None)), (Ok(Some(1.0)), failed())] {
            let (still, looked) = activation(
                before(showing, was.clone()),
                now(showing, is.clone(), "Save"),
            );
            assert_eq!(still.observed, Observed::Present);
            assert_eq!(still.effect, Effect::Unverified);
            assert!(still.detail.unwrap().contains("no answer"));
            assert_eq!(looked.value_changed, None);
            let (toggled, _) = activation(before(showing, was), now(checked, is, "Save"));
            assert_eq!(toggled.effect, Effect::Seen);
            assert_eq!(toggled.detail, None);
        }
    }

    // A field that truncates, caps or reformats what it is given holds other text: that is
    // `differs`, never the success a matching count alone used to give.
    #[test]
    fn written_text_is_matched_only_when_the_field_holds_it() {
        let (same, characters) = written(Ok(Held::Same(4)));
        assert_eq!(same, Seen::new(Observed::Matched, Effect::Seen, None));
        assert_eq!(characters, Some(4));
        for held in [Held::Other(2), Held::Other(4), Held::Other(-1)] {
            let (seen, _) = written(Ok(held));
            assert_eq!(seen, Seen::new(Observed::Differs, Effect::Differs, None));
        }
        let refused = ToolError::new(ErrorName::UpstreamError, "no Text interface");
        assert_eq!(
            written(Err(Failed::Refused(refused))),
            (
                Seen::new(
                    Observed::Unknown,
                    Effect::Unverified,
                    Some("no Text interface".to_owned())
                ),
                None
            )
        );
    }

    #[test]
    fn text_over_64_kib_of_utf8_is_refused_before_anything_is_asked() {
        assert!(check_text(&"a".repeat(MAX_TEXT)).is_ok());
        assert!(check_text("").is_ok());
        // 21846 three-byte characters are 65538 bytes.
        let over = "€".repeat(21_846);
        assert!(matches!(
            check_text(&over),
            Err(CallError::Tool(error)) if error.name == ErrorName::TextTooLong
        ));
    }

    #[test]
    fn the_log_keeps_the_role_actions_kind_and_index_and_length_never_a_name_or_text() {
        let button = element(3, &["SECRET-1", "Press"]);
        let expect = json!({"window_id": 3});
        assert_eq!(
            logged("elem-0a1b2c3d-1", Some(&button), None, None, &expect),
            json!({"element": "elem-0a1b2c3d-1", "role": "button", "action": "press", "action_index": 1, "expect": expect})
        );
        assert_eq!(
            logged(
                "elem-0a1b2c3d-1",
                Some(&button),
                Some("SECRET-1"),
                None,
                &expect
            ),
            json!({"element": "elem-0a1b2c3d-1", "role": "button", "action": "other", "action_index": 0, "expect": expect})
        );
        // An action the element didn't list, or an unknown ref, logs the kind asked for.
        assert_eq!(
            logged(
                "elem-0a1b2c3d-1",
                Some(&button),
                Some("SECRET-2"),
                None,
                &expect
            ),
            json!({"element": "elem-0a1b2c3d-1", "role": "button", "action": "other", "action_index": null, "expect": expect})
        );
        assert_eq!(
            logged("elem-0a1b2c3d-9", None, Some("Click"), None, &json!("none")),
            json!({"element": "elem-0a1b2c3d-9", "role": null, "action": "click", "action_index": null, "expect": "none"})
        );
        assert_eq!(
            logged(
                "elem-0a1b2c3d-1",
                Some(&button),
                None,
                Some(12),
                &json!({"app_id_len": 1})
            ),
            json!({"element": "elem-0a1b2c3d-1", "role": "button", "text_len": 12, "expect": {"app_id_len": 1}})
        );
        // A ref not in the form refs are issued in is the agent's text.
        assert_eq!(
            logged("SECRET-3", None, None, Some(0), &json!("none"))["element"],
            json!({"category": "invalid_ref", "bytes": 8})
        );
    }
}
