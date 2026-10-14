//! The desktop engine: what a niri instance's clients share, apart from any transport. It
//! owns the crash guardian, the event stream, the accessibility bus, the audit log and the
//! desk, and runs each tool's work through them: the readiness check, the action gate with
//! its evidence, captures and their refs, waits, and giving focus back. `tools.rs` turns
//! MCP calls into calls here and the results into MCP content.

pub(crate) mod hello;
pub(crate) mod host;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;
use serde_json::Value;
use tokio::time::Instant;

use crate::a11y::{self, A11y, Presence};
use crate::act::record::{Quiet, Recorder};
use crate::act::{self, Outcome};
use crate::audit::Audit;
use crate::changes::Changes;
use crate::changes::elements::{self as element_changes, Seen, Unknown};
use crate::control;
use crate::control::desk::{Desk, Prepared, Worked};
use crate::control::lease::Holder;
use crate::coords::ImageRect;
use crate::error::{CallError, ErrorName, ToolError};
use crate::input::Input;
use crate::niri::OutputSummary;
use crate::niri::events::{DesktopState, EventStream, StreamState};
use crate::observe;
use crate::policy::{self, SaveTarget};
use crate::refs::Shot;
use crate::session::{Label, Session, SessionId, Settings};
use crate::{Env, clipboard, confirm, elements, niri, noctalia, runner, settle, status, wait};

#[derive(Debug)]
pub(crate) struct Engine {
    env: Env,
    /// Without niri's socket, the socket's error.
    events: Result<EventStream, ToolError>,
    audit: Audit,
    desk: Desk,
    /// Whether there is an accessibility bus, decided once at startup for the same reason.
    accessibility: Presence,
    /// The accessibility bus, when there is one.
    a11y: Option<A11y>,
    /// The sessions being served, by id.
    sessions: Mutex<BTreeMap<SessionId, Session>>,
    /// The clipboard as the current lease sees it, from its `acquire` on.
    clipboard: Mutex<Option<clipboard::Watch>>,
    /// The last session id given out.
    last_session: AtomicU64,
    mode: status::Mode,
    /// Why a standalone engine's client isn't served by the shared engine it asked for.
    fallback: Option<String>,
    /// Why a shared engine runs without a scope of its own, as the bridge that started it
    /// said.
    unscoped: Option<String>,
    /// Lives as long as the engine; see `control::guard`.
    _guardian: Option<runner::Watcher>,
}

/// The action gate's checks for one session, which an element action asks again while
/// it runs.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Recheck<'a> {
    engine: &'a Engine,
    session: &'a Session,
}

impl elements::actions::Recheck for Recheck<'_> {
    async fn control(&self) -> Result<(), ToolError> {
        // Boxed, because the readiness report makes a large future.
        Box::pin(self.engine.refusal(self.session))
            .await
            .map_or(Ok(()), Err)
    }

    fn desk(&self) -> Result<(), ToolError> {
        self.engine.desk.checkpoint(self.session)
    }

    fn sending(&self) -> Result<(), ToolError> {
        self.engine.desk.sending()
    }

    fn answered(&self, outcome: Outcome) {
        self.engine.desk.answered(outcome);
    }
}

/// What `desktop_state` returns.
#[derive(Debug, Serialize)]
pub(crate) struct Desktop {
    #[serde(flatten)]
    state: DesktopState,
    /// By name.
    outputs: Vec<OutputSummary>,
    /// Only when Noctalia is installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    shell: Option<Shell>,
}

/// Noctalia's status as it gave it, or why it gave none.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum Shell {
    Status(serde_json::Map<String, Value>),
    Unavailable(ToolError),
}

/// A zoom into an earlier screenshot: its ref and the pixels of its image to capture.
#[derive(Debug)]
pub(crate) struct Zoom {
    pub(crate) id: String,
    pub(crate) pixels: ImageRect,
}

/// What `release_desktop` returns.
#[derive(Debug, Serialize)]
pub(crate) struct Release {
    pub(crate) users_window: Option<u64>,
    /// How giving focus back went, when it was asked for and there was a window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) restored: Option<Value>,
    pub(crate) released: bool,
}

impl Engine {
    /// Starts the crash guardian before anything else, then niri's event stream, and looks
    /// for the accessibility bus.
    pub(crate) async fn start(
        env: Env,
        mode: status::Mode,
        fallback: Option<String>,
    ) -> Result<Self, String> {
        // `/proc/self/exe` still runs this binary when its file has been replaced since.
        let guardian = runner::watcher(
            "/proc/self/exe",
            &["guard".to_owned(), std::process::id().to_string()],
        )
        .map_err(|error| format!("start the crash guardian: {}", error.detail))?;
        let events = env
            .niri_socket
            .path()
            .map(|socket| EventStream::spawn(socket.to_path_buf()))
            .map_err(Clone::clone);
        let audit = Audit::new(env.state_dir.clone());
        let accessibility = a11y::detect(env.session_bus.as_deref()).await;
        Ok(Self {
            _guardian: Some(guardian),
            mode,
            fallback,
            ..Self::new(env, events, audit, accessibility)
        })
    }

    /// A standalone engine without a crash guardian, from resources already made.
    pub(crate) fn new(
        env: Env,
        events: Result<EventStream, ToolError>,
        audit: Audit,
        accessibility: Presence,
    ) -> Self {
        let a11y = accessibility.address.clone().map(A11y::new);
        Self {
            desk: Desk::start(&env),
            env,
            events,
            audit,
            accessibility,
            a11y,
            sessions: Mutex::new(BTreeMap::new()),
            clipboard: Mutex::new(None),
            last_session: AtomicU64::new(0),
            mode: status::Mode::Standalone,
            fallback: None,
            unscoped: None,
            _guardian: None,
        }
    }

    /// Starts serving a session for the client process `pid`, with `settings`.
    pub(crate) fn open_session(&self, pid: u32, settings: Settings) -> Session {
        let id = SessionId(self.last_session.fetch_add(1, Ordering::Relaxed) + 1);
        let session = Session::new(id, pid, settings);
        self.sessions().insert(id, session.clone());
        session
    }

    /// Ends `session`, if it hasn't ended yet, and stops counting it.
    pub(crate) async fn close_session(&self, session: &Session) {
        self.end_session(session).await;
        self.sessions().remove(&session.id());
    }

    /// Ends every session being served.
    pub(crate) async fn end_sessions(&self) {
        let sessions: Vec<Session> = self.sessions().values().cloned().collect();
        for session in &sessions {
            self.end_session(session).await;
        }
    }

    /// Whether the engine has nothing to finish: no session holds the lease and no input
    /// cleanup is pending.
    pub(crate) fn settled(&self) -> bool {
        !self.desk.held() && control::cleanup::pending() == 0
    }

    /// Returns once the stop watcher has ended, for good: never, without a watcher.
    pub(crate) async fn watcher_ended(&self) {
        self.desk.watcher_ended().await;
    }

    /// No code panics while holding the lock, so a poisoned one still holds sound entries.
    fn sessions(&self) -> std::sync::MutexGuard<'_, BTreeMap<SessionId, Session>> {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) const fn env(&self) -> &Env {
        &self.env
    }

    pub(crate) const fn audit(&self) -> &Audit {
        &self.audit
    }

    /// Found once at startup, because the tool list depends on it.
    pub(crate) const fn noctalia_installed(&self) -> bool {
        self.env.desktop.noctalia.is_some()
    }

    pub(crate) const fn has_a11y(&self) -> bool {
        self.a11y.is_some()
    }

    pub(crate) const fn niri(&self) -> act::Niri<'_> {
        act::Niri {
            socket: &self.env.niri_socket,
            events: self.events.as_ref(),
            desk: &self.desk,
            locks: self.lock_sources(),
        }
    }

    /// What the input tools work with, for `session`.
    pub(crate) fn input<'a>(&'a self, session: &'a Session) -> Result<Input<'a>, ToolError> {
        let settings = session.settings();
        Ok(Input {
            niri: self.niri(),
            display: &self.env.display,
            runtime: self.desk.runtime()?,
            policy: &settings.policy,
            a11y: self.a11y.as_ref(),
        })
    }

    /// The action gate's checks for `session`, to ask again while its action runs.
    pub(crate) const fn recheck<'a>(&'a self, session: &'a Session) -> Recheck<'a> {
        Recheck {
            engine: self,
            session,
        }
    }

    /// The screenshot ref named `id` of the lease `session` holds.
    pub(crate) fn shot(&self, session: &Session, id: &str) -> Result<Shot, ToolError> {
        self.desk.shot(session, id)
    }

    /// The element ref named `id` of the lease `session` holds.
    pub(crate) fn element(
        &self,
        session: &Session,
        id: &str,
    ) -> Result<a11y::ElementRef, ToolError> {
        self.desk.element(session, id)
    }

    /// The readiness report, as `status` returns it to `session`.
    pub(crate) async fn status(&self, session: &Session) -> status::Status {
        let settings = session.settings();
        let event_stream = self
            .events
            .as_ref()
            .map_or(StreamState::Disconnected, EventStream::state);
        let engine = status::EngineStatus {
            pid: std::process::id(),
            mode: self.mode,
            sessions: self.sessions().len(),
            fallback: self.fallback.clone(),
            unscoped: self.unscoped.clone(),
            version: env!("CARGO_PKG_VERSION"),
            build: hello::Build::current(),
            cgroup: status::own_cgroup(),
        };
        let sources = status::Sources {
            engine: Some(engine),
            event_stream: Some(event_stream),
            audit: &self.audit,
            lease: self.desk.status(session),
            policy: &settings.policy,
            unrestricted: &settings.unrestricted,
            confirm_lease: &settings.confirm_lease,
            tools: &settings.tools,
            place: settings.place(),
            accessibility: &self.accessibility,
        };
        Box::pin(status::collect(&self.env, sources)).await
    }

    /// Takes the lease for `session`, labelled `label`, and watches the clipboard from
    /// then on. Returns the holder and the window that had keyboard focus, which `release`
    /// can give focus back to.
    pub(crate) async fn acquire(
        self: &Arc<Self>,
        session: &Session,
        label: &Label,
    ) -> Result<(Holder, Option<u64>), ToolError> {
        // Boxed, because the readiness report and the prompt make a large future.
        let refusal = Box::pin(self.acquire_refusal(session, label));
        self.grant(session, &label.to_string(), refusal).await
    }

    /// Why `session`'s client `label` may not take the lease: the readiness report's
    /// refusal, or with `confirm_lease` the user's answer on the desktop. Since the user
    /// may take up to `confirm::DEADLINE` to answer, readiness is asked again after an Allow.
    async fn acquire_refusal(&self, session: &Session, label: &Label) -> Option<ToolError> {
        if let Some(refusal) = Box::pin(self.refusal(session)).await {
            return Some(refusal);
        }
        let settings = session.settings();
        if !settings.confirm_lease.enabled() {
            return None;
        }
        if let Err(declined) = confirm::ask(settings.place(), label).await {
            return Some(declined);
        }
        Box::pin(self.refusal(session)).await
    }

    /// `acquire` with `refusal` as the policy's answer. The clipboard watcher is bound
    /// before the lease is granted and kept with no await after the grant, so a call
    /// dropped at any point leaves either no lease or the lease with its watcher.
    async fn grant(
        self: &Arc<Self>,
        session: &Session,
        label: &str,
        refusal: impl Future<Output = Option<ToolError>>,
    ) -> Result<(Holder, Option<u64>), ToolError> {
        let prepare = |lease| async move {
            let users_window = niri::waiter(self.events.as_ref())
                .await
                .ok()
                .and_then(|waiter| waiter.view().focused_window());
            Prepared {
                users_window,
                with: self.bind_watch(session, lease).await,
            }
        };
        let (holder, watch) = self
            .desk
            .acquire_with(session, label, refusal, prepare)
            .await?;
        if let Some(watch) = watch {
            self.keep_watch(session, watch)?;
        }
        Ok((holder, self.desk.users_window(session)))
    }

    /// Gives the lease up, with `restore_focus` first giving focus back to the user's
    /// window through the action gate.
    pub(crate) async fn release(&self, session: &Session, restore_focus: bool) -> Release {
        let users_window = self.desk.users_window(session);
        let restored = match (restore_focus, users_window) {
            (true, Some(id)) => Some(self.restore(session, id).await),
            _ => None,
        };
        Release {
            users_window,
            restored,
            released: self.desk.release(session).await,
        }
    }

    /// Ends `session` once its client has gone: drops its running action and gives its
    /// lease up, without giving focus back.
    pub(crate) async fn end_session(&self, session: &Session) {
        self.desk.end_session(session).await;
    }

    /// Runs one action through the desk's gate, with a screenshot when `report.shoot` asks
    /// for one or its outcome is in doubt, and the report of what it changed.
    pub(crate) async fn act(
        &self,
        session: &Session,
        report: Report<'_>,
        work: impl Future<Output = Result<Outcome, CallError>>,
    ) -> Result<act::Evidenced, CallError> {
        // Boxed, because the readiness report, the action's work and its wait make large
        // futures.
        let refusal = Box::pin(self.refusal(session));
        let watched = async {
            let before = match report.elements {
                Some(target) => Some(Box::pin(self.survey_before(session, target)).await),
                None => None,
            };
            let mut recorder = Recorder::start(self.events.as_ref()).await;
            let outcome = match &mut recorder {
                Ok(recorder) => recorder.follow(work).await?,
                Err(_) => work.await?,
            };
            Ok(Watched {
                outcome,
                recorder,
                before,
            })
        };
        let evidence =
            |worked| Box::pin(self.evidence(session, worked, report.shoot, report.quiet));
        self.desk
            .act(session, refusal, Box::pin(watched), evidence)
            .await
    }

    /// A screenshot, serialized with this engine's actions. With `save`, a full-resolution
    /// PNG of the target is written there first. With `zoom`, the target is the region of
    /// an earlier screenshot it names, resolved once the screenshot's turn has come.
    pub(crate) async fn screenshot(
        &self,
        session: &Session,
        mut request: observe::Request,
        save: Option<SaveTarget>,
        zoom: Option<Zoom>,
    ) -> Result<observe::Screenshot, CallError> {
        let capture = async {
            if let Some(zoom) = zoom {
                let (region, pin) = self.zoomed(session, &zoom)?;
                request.target = observe::Target::Region(region);
                request.pin = Some(pin);
            }
            // Boxed, because a saved capture makes a large future.
            Box::pin(self.capture_saving(session, request, save)).await
        };
        self.desk.observe(session, capture).await
    }

    /// The layout rectangle `zoom` names and the output to capture it from: the ref must be
    /// one of the lease `session` holds or, without the lease, one `session` took since it
    /// last held it, and still fresh.
    fn zoomed(
        &self,
        session: &Session,
        zoom: &Zoom,
    ) -> Result<(observe::Rect, observe::Pin), ToolError> {
        let shot = self.desk.zoomable(session, &zoom.id)?;
        let connection = self.events.as_ref().ok().and_then(EventStream::connection);
        shot.zoom(zoom.pixels, Instant::now(), connection)
    }

    /// The accessible elements `ask` names, each kept as an element ref while the lease is
    /// held.
    pub(crate) async fn elements(
        &self,
        session: &Session,
        ask: &elements::Ask,
    ) -> Result<elements::Listing, CallError> {
        let Some(a11y) = &self.a11y else {
            return Err(
                a11y::not_accessible("this session has no accessibility bus".to_owned()).into(),
            );
        };
        let lease = self.desk.ref_lease(session);
        let remember = |element| lease.and_then(|lease| self.desk.remember_element(lease, element));
        // Boxed, because the walk's calls make a large future.
        Box::pin(elements::list(
            &self.env.niri_socket,
            a11y,
            &session.settings().policy,
            ask,
            remember,
        ))
        .await
    }

    /// What `elements` lists for `element` now, for one check of `wait_for`'s element
    /// condition. While the lease is held, an appearing element is kept as an element ref.
    /// An `absent` check lists nothing, only whether anything matches: it would throw the
    /// element away, and naming its actions and keeping its ref on each walk that still
    /// sees it would cost time and push the agent's own refs out of those the lease keeps.
    /// The walk and its naming end by `by`, giving what they read. Without the lease no
    /// ref is withheld, since none would be kept.
    async fn poll(
        &self,
        session: &Session,
        element: &wait::Element,
        by: Instant,
    ) -> Result<Option<elements::Listing>, ToolError> {
        let Some(a11y) = &self.a11y else {
            return Err(a11y::not_accessible(
                "this session has no accessibility bus".to_owned(),
            ));
        };
        let ask = elements::Ask {
            window_id: element.window_id,
            filter: element.filter.clone(),
            limit: usize::from(!element.absent),
            whole: !element.absent,
            by: Some(by),
        };
        let lease = self.desk.ref_lease(session).filter(|_| !element.absent);
        let remember = |found| lease.and_then(|lease| self.desk.remember_element(lease, found));
        // Boxed, because the walk's calls make a large future.
        let polled = Box::pin(elements::poll(
            &self.env.niri_socket,
            a11y,
            &session.settings().policy,
            &ask,
            remember,
        ))
        .await;
        polled.map(|listing| {
            listing.map(|mut listing| {
                listing.withheld &= lease.is_some();
                listing
            })
        })
    }

    /// One snapshot of the event stream, with niri's outputs and, when Noctalia is
    /// installed, its status or why it has none.
    pub(crate) async fn desktop(&self) -> Result<Desktop, ToolError> {
        let shell = async {
            if self.noctalia_installed() {
                Some(noctalia::status(&self.env).await)
            } else {
                None
            }
        };
        let (state, outputs, shell) = tokio::join!(
            niri::desktop(self.events.as_ref()),
            niri::outputs(&self.env.niri_socket),
            shell
        );
        Ok(Desktop {
            state: state?,
            outputs: outputs?.into_values().map(Into::into).collect(),
            shell: shell.map(|status| match status {
                Ok(status) => Shell::Status(status),
                Err(error) => Shell::Unavailable(error),
            }),
        })
    }

    /// The clipboard's text, if it was copied during `session`'s lease.
    pub(crate) async fn clipboard(&self, session: &Session) -> Result<impl Serialize, ToolError> {
        let lease = self.desk.ref_lease(session).ok_or_else(|| {
            ToolError::new(
                ErrorName::LeaseRequired,
                "this session doesn't hold the lease; call acquire_desktop first",
            )
        })?;
        let reader = self
            .watch()
            .as_ref()
            .filter(|watch| watch.lease() == lease)
            .map(clipboard::Watch::reader)
            .ok_or_else(|| {
                ToolError::new(
                    ErrorName::UpstreamError,
                    "nothing watches the clipboard for this lease; release the desktop and acquire it again",
                )
            })?;
        reader.read().await
    }

    /// A clipboard watcher for `session`'s `lease`, bound while the lease is about to be
    /// granted. It ends itself once the lease is gone, however it went: released, taken back
    /// by a stop, lost, or its session ended.
    async fn bind_watch(self: &Arc<Self>, session: &Session, lease: u64) -> clipboard::Watch {
        let engine = Arc::downgrade(self);
        let owner = session.clone();
        let holds = move || {
            engine
                .upgrade()
                .is_some_and(|engine| engine.desk.pending(lease) || engine.holds(&owner, lease))
        };
        let (socket, display) = (self.env.niri_socket.clone(), self.env.display.clone());
        clipboard::Watch::start(socket, display, lease, holds).await
    }

    /// Whether `session` is still served and holds `lease`.
    fn holds(&self, session: &Session, lease: u64) -> bool {
        !session.has_ended() && self.desk.ref_lease(session) == Some(lease)
    }

    /// Keeps `watch` as the clipboard's watcher, in place of an earlier lease's, if
    /// `session` still holds its lease. Checked with the slot locked, so a watcher for a
    /// lease that has ended since never replaces a later lease's; `session` is then told
    /// its lease is gone.
    fn keep_watch(&self, session: &Session, watch: clipboard::Watch) -> Result<(), ToolError> {
        let mut current = self.watch();
        if !self.holds(session, watch.lease()) {
            return Err(lease_gone(session));
        }
        *current = Some(watch);
        drop(current);
        Ok(())
    }

    fn watch(&self) -> std::sync::MutexGuard<'_, Option<clipboard::Watch>> {
        self.clipboard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for `until`, then with `screenshot` adds one taken once the screen stopped
    /// changing. Each capture serializes with actions, without holding the mutex between
    /// samples or while waiting for a window condition.
    pub(crate) async fn wait(
        &self,
        session: &Session,
        until: &wait::Until,
        limit: std::time::Duration,
        screenshot: bool,
    ) -> Result<(wait::Report, Option<observe::Screenshot>), CallError> {
        let capture = || {
            let capture = self.capture(session, observe::Request::focused());
            self.desk.observe(session, capture)
        };
        if *until == wait::Until::ScreenStable {
            let (report, last) = wait::screen(capture, limit).await?;
            return Ok((report, screenshot.then_some(last)));
        }
        let report = if let wait::Until::Element(element) = until {
            let poll = |by| self.poll(session, element, by);
            wait::element(self.events.as_ref(), element, limit, poll).await?
        } else {
            wait::window(self.events.as_ref(), until, limit).await?
        };
        if !screenshot {
            return Ok((report, None));
        }
        match settle::screenshot(capture, settle::LIMIT).await {
            Ok(shot) => Ok((report, Some(shot))),
            Err(CallError::Tool(error)) => Ok((
                wait::Report {
                    screenshot_error: Some(error),
                    ..report
                },
                None,
            )),
            Err(mistake @ CallError::InvalidArguments(_)) => Err(mistake),
        }
    }

    /// Focuses the user's window `id` through the action gate, as the outcome or the error.
    async fn restore(&self, session: &Session, id: u64) -> Value {
        let refusal = Box::pin(self.refusal(session));
        let work = Box::pin(act::refocus(self.niri(), id));
        let concluded = |worked| std::future::ready(Outcome::concluded(worked));
        let acted = self.desk.act(session, refusal, work, concluded);
        let restored = match acted.await {
            Ok(outcome) => serde_json::to_value(outcome),
            Err(CallError::Tool(error)) => serde_json::to_value(error),
            Err(CallError::InvalidArguments(message)) => {
                serde_json::to_value(ToolError::new(ErrorName::UpstreamError, message))
            }
        };
        restored.unwrap_or(Value::Null)
    }

    /// Takes a screenshot and keeps it as a ref that the result names: one of the lease
    /// while `session` holds it, and otherwise one that only a zoom takes.
    async fn capture(
        &self,
        session: &Session,
        request: observe::Request,
    ) -> Result<observe::Screenshot, CallError> {
        let lease = self.desk.ref_lease(session);
        let stretch = self.desk.stretch(session);
        let connection = self.events.as_ref().ok().and_then(EventStream::connection);
        let taken = Instant::now();
        let place = session.settings().place();
        let mut shot =
            observe::screenshot(&self.env.niri_socket, &self.env.display, place, &request).await?;
        let Some(connection) = connection else {
            return Ok(shot);
        };
        let kept = Shot::of(&shot, taken, connection);
        shot.metadata.screenshot_ref = match lease {
            Some(lease) => self.desk.remember(lease, kept),
            None => self.desk.remember_view(session, stretch, kept),
        };
        Ok(shot)
    }

    /// With `save`, first writes a full-resolution PNG of the target there; then captures
    /// the screenshot to return, which says where the PNG went.
    async fn capture_saving(
        &self,
        session: &Session,
        request: observe::Request,
        save: Option<SaveTarget>,
    ) -> Result<observe::Screenshot, CallError> {
        let saved = match &save {
            Some(save) => {
                let (socket, display) = (&self.env.niri_socket, &self.env.display);
                let place = session.settings().place();
                Some(observe::save(socket, display, place, &request, save).await?)
            }
            None => None,
        };
        let mut shot = self.capture(session, request).await?;
        shot.metadata.saved = saved;
        Ok(shot)
    }

    /// Where the lock state is read.
    const fn lock_sources(&self) -> control::LockSources<'_> {
        control::LockSources {
            env: &self.env,
            noctalia: self.noctalia_installed(),
        }
    }

    /// Why `session` may not take the lease or act now, from the readiness report.
    async fn refusal(&self, session: &Session) -> Option<ToolError> {
        let report = self.status(session).await;
        if let Some(refused) = policy::refuse_control(report.facts(&session.settings().policy)) {
            return Some(refused);
        }
        // The report asks Noctalia beside niri, so its answer may be older than niri's.
        // The lock decision comes from reads started once the report is done, and a call
        // that acts goes out only within `LAST_GATE` of when they began.
        let began = Instant::now();
        let lock = self.lock_sources().read().await;
        self.desk.lock_read(began);
        policy::refuse_lock(lock.state())
    }

    /// The outcome with the screenshot `shoot` asks for, or the one an outcome in doubt
    /// gets, then what the action changed, waiting for quiet as `quiet` says. For work cut
    /// short after its call went out, the outcome alone: after a stop or with the lease
    /// gone, nothing more is asked.
    async fn evidence(
        &self,
        session: &Session,
        worked: Worked<Watched>,
        shoot: bool,
        quiet: Quiet,
    ) -> act::Evidenced {
        let watched = match worked {
            Worked::Done(watched) => watched,
            Worked::CutAfterSending(cut, answered) => {
                return act::Evidenced {
                    outcome: Outcome::concluded(Worked::CutAfterSending(cut, answered)),
                    image: None,
                };
            }
        };
        let Watched {
            outcome,
            mut recorder,
            before,
        } = watched;
        let capture = |request| self.capture(session, request);
        let evidence = act::with_evidence(outcome, shoot, capture);
        let mut evidenced = match &mut recorder {
            Ok(recorder) => recorder.follow(evidence).await,
            Err(_) => evidence.await,
        };
        let wait = quiet.wait(shoot, evidenced.outcome.observed);
        let mut changes = match recorder {
            Ok(recorder) => recorder.finish(wait, &mut evidenced.outcome).await,
            // Nothing was watched, so an empty report would read as "nothing changed".
            Err(error) => Changes {
                lost: Some(error.detail),
                ..Changes::default()
            },
        };
        if let Some(before) = before {
            changes.elements = Box::pin(self.survey_after(session, before)).await;
        }
        evidenced.outcome.report(changes);
        evidenced
    }

    /// The elements of the window `target` names, before an action, for its report.
    async fn survey_before(&self, session: &Session, target: Watch<'_>) -> Surveyed {
        let window = match target {
            Watch::Element(id) => match self.desk.element(session, id) {
                Ok(element) => element.kept.window,
                Err(error) => return Err(Unknown::failed(None, error)),
            },
            Watch::Focused => match niri::waiter(self.events.as_ref()).await {
                Ok(waiter) => waiter
                    .view()
                    .focused_window()
                    .ok_or_else(Unknown::no_window)?,
                Err(error) => return Err(Unknown::failed(None, error)),
            },
        };
        let seen = self.survey(session, window).await?;
        Ok((window, seen))
    }

    /// What changed among the elements surveyed `before` the action, from another walk.
    async fn survey_after(
        &self,
        session: &Session,
        before: Surveyed,
    ) -> Option<element_changes::Report> {
        let (window, before) = match before {
            Ok(before) => before,
            Err(unknown) => return Some(element_changes::Report::Unknown(unknown)),
        };
        let mut after = match self.survey(session, window).await {
            Ok(after) => after,
            Err(unknown) => return Some(element_changes::Report::Unknown(unknown)),
        };
        let lease = self.desk.ref_lease(session);
        if let (Some(a11y), Some(_)) = (&self.a11y, lease) {
            let reported = element_changes::reported(&before, &after);
            elements::name_reported(a11y, &mut after, &reported).await;
        }
        let remember = |seen: &Seen| {
            lease.and_then(|lease| self.desk.remember_element(lease, seen.element.clone()))
        };
        element_changes::diff(window, &before, &after, remember).map(element_changes::Report::Diff)
    }

    /// Every element `elements` would list of window `window`.
    async fn survey(&self, session: &Session, window: u64) -> Result<Vec<Seen>, Unknown> {
        let Some(a11y) = &self.a11y else {
            let none = a11y::not_accessible("this session has no accessibility bus".to_owned());
            return Err(Unknown::failed(Some(window), none));
        };
        let policy = &session.settings().policy;
        Box::pin(elements::survey(
            &self.env.niri_socket,
            a11y,
            policy,
            window,
        ))
        .await
    }
}

/// What an action's report covers: whether a screenshot was asked for, the window whose
/// elements to compare before and after, if any, and whether it waits for quiet.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Report<'a> {
    pub(crate) shoot: bool,
    pub(crate) elements: Option<Watch<'a>>,
    pub(crate) quiet: Quiet,
}

impl Report<'_> {
    /// A report of niri's changes alone, with a screenshot when `shoot`.
    pub(crate) const fn shot(shoot: bool) -> Self {
        Self {
            shoot,
            elements: None,
            quiet: Quiet::Always,
        }
    }

    /// As `shot`, waiting for quiet as `quiet` says.
    pub(crate) const fn quiet(shoot: bool, quiet: Quiet) -> Self {
        Self {
            shoot,
            elements: None,
            quiet,
        }
    }
}

/// Whose elements an action's report compares.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Watch<'a> {
    /// The window of the element ref with this id.
    Element(&'a str),
    /// The window with keyboard focus when the action starts.
    Focused,
}

/// A window's elements before an action, or why they aren't known.
type Surveyed = Result<(u64, Vec<Seen>), Unknown>;

/// An action's outcome, with the watch on niri, or why it couldn't start, and the elements
/// from before it.
#[derive(Debug)]
struct Watched {
    outcome: Outcome,
    recorder: Result<Recorder, ToolError>,
    before: Option<Surveyed>,
}

/// Why `acquire_desktop` fails when `session`'s lease went before the clipboard watch for it
/// was bound.
fn lease_gone(session: &Session) -> ToolError {
    let detail = if session.has_ended() {
        "this session ended before acquire_desktop finished, and its lease was given up"
    } else {
        "the lease was given up before acquire_desktop finished: released, taken back by a stop, or its file moved; call acquire_desktop again if you still need it"
    };
    ToolError::new(ErrorName::LeaseRequired, detail)
}

#[cfg(test)]
mod tests {
    use std::future::ready;
    use std::time::Duration;

    use super::*;
    use crate::RuntimeDir;

    #[tokio::test]
    async fn another_session_releasing_neither_refocuses_nor_learns_the_owners_window() {
        let dir = crate::test_support::fresh_dir("engine-release");
        let engine = without_niri(crate::test_support::niri_env(&dir));
        let owner = crate::test_support::session(1);
        engine
            .desk
            .acquire(&owner, "owner/1", ready(None), ready(Some(5)))
            .await
            .unwrap();
        let other = crate::test_support::session(2);
        let release = engine.release(&other, true).await;
        assert_eq!(release.users_window, None);
        assert!(release.restored.is_none(), "{:?}", release.restored);
        assert!(!release.released);
        assert!(engine.desk.status(&owner).held_by_me);
        assert_eq!(engine.desk.users_window(&owner), Some(5));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An acquisition dropped while its clipboard watch binds leaves no lease, so another
    /// session can take it and read what it copies.
    #[tokio::test]
    async fn an_acquire_dropped_while_its_watch_binds_leaves_no_lease() {
        use crate::test_support::wayland::Compositor;

        let dir = crate::test_support::fresh_dir("engine-dropped-acquire");
        let niri = Compositor::start(&dir, include_str!("../tests/fixtures/us-de.xkb"));
        RuntimeDir::of(&niri.env(&dir)).unwrap().create().unwrap();
        let engine = Arc::new(without_niri(niri.env(&dir)));
        niri.hold_client().await;
        let dropped = grant_apart(&engine, &crate::test_support::session(1));
        until(|| niri.seen().holding).await;
        dropped.abort();
        assert!(dropped.await.unwrap_err().is_cancelled());
        assert!(!engine.desk.held());
        niri.serve_held().await;
        let next = crate::test_support::session(2);
        engine.grant(&next, "next/2", ready(None)).await.unwrap();
        assert_eq!(copied_and_read(&niri, &engine, &next).await, "copied");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A watch that ticks while its lease is still to be granted keeps watching once it is.
    #[tokio::test]
    async fn a_watch_bound_before_its_lease_is_granted_keeps_watching() {
        use crate::test_support::wayland::Compositor;

        let dir = crate::test_support::fresh_dir("engine-pending-watch");
        let niri = Compositor::start(&dir, include_str!("../tests/fixtures/us-de.xkb"));
        RuntimeDir::of(&niri.env(&dir)).unwrap().create().unwrap();
        let engine = Arc::new(without_niri(niri.env(&dir)));
        let session = crate::test_support::session(1);
        let watch = engine.bind_watch(&session, 1).await;
        // Past the watcher's first two checks of its lease.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let granted = engine
            .desk
            .acquire(&session, "test/1", ready(None), ready(None));
        granted.await.unwrap();
        engine.keep_watch(&session, watch).unwrap();
        assert_eq!(copied_and_read(&niri, &engine, &session).await, "copied");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// How the first session's lease ends while its clipboard watch is binding.
    #[derive(Debug, Clone, Copy)]
    enum Ending {
        Release,
        Disconnect,
        Stop,
    }

    impl Ending {
        /// What the acquisition binding meanwhile is refused with: a release waits for it
        /// and gives the lease up after.
        const fn refusal(self) -> Option<ErrorName> {
            match self {
                Self::Release => None,
                Self::Disconnect => Some(ErrorName::LeaseRequired),
                Self::Stop => Some(ErrorName::Stopped),
            }
        }
    }

    /// A lease that ends while its acquisition binds the clipboard watch leaves the next
    /// lease a watch of its own.
    #[tokio::test]
    async fn a_lease_ended_while_its_watch_binds_leaves_the_next_lease_a_watch() {
        for ending in [Ending::Release, Ending::Disconnect, Ending::Stop] {
            ended_while_binding(ending).await;
        }
    }

    async fn ended_while_binding(ending: Ending) {
        use crate::test_support::wayland::Compositor;

        let dir = crate::test_support::fresh_dir(&format!("engine-ended-binding-{ending:?}"));
        let niri = Compositor::start(&dir, include_str!("../tests/fixtures/us-de.xkb"));
        let runtime = RuntimeDir::of(&niri.env(&dir)).unwrap();
        runtime.create().unwrap();
        let engine = Arc::new(without_niri(niri.env(&dir)));
        let first = crate::test_support::session(1);
        niri.hold_client().await;
        let binding = grant_apart(&engine, &first);
        until(|| niri.seen().holding).await;
        let ender = end_apart(&engine, &runtime, &first, ending).await;
        niri.serve_held().await;
        let first_got = binding.await.unwrap().map(|_| ());
        ender.await.unwrap();
        if matches!(ending, Ending::Stop) {
            runtime.resume().unwrap();
        }
        let refused = first_got.err().map(|error| error.name);
        assert_eq!(refused, ending.refusal(), "{ending:?}");
        assert!(!engine.desk.held(), "{ending:?}");
        let next = crate::test_support::session(2);
        granted_soon(&engine, &next).await;
        assert_eq!(copied_and_read(&niri, &engine, &next).await, "copied");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Starts ending `session`'s lease as `ending` says, in a task of its own, since that
    /// waits for the desk; returns once a disconnect has ended the session. A release must
    /// find the lease and give it up.
    async fn end_apart(
        engine: &Arc<Engine>,
        runtime: &RuntimeDir,
        session: &Session,
        ending: Ending,
    ) -> tokio::task::JoinHandle<()> {
        if matches!(ending, Ending::Stop) {
            runtime.stop().unwrap();
        }
        let (engine, owner) = (Arc::clone(engine), session.clone());
        let ender = tokio::spawn(async move {
            match ending {
                Ending::Release => assert!(engine.release(&owner, false).await.released),
                Ending::Disconnect => engine.close_session(&owner).await,
                Ending::Stop => {}
            }
        });
        if matches!(ending, Ending::Disconnect) {
            until(|| session.has_ended()).await;
        }
        ender
    }

    /// The holder acquiring again keeps the lease's watch, so a copy noted before stays
    /// readable.
    #[tokio::test]
    async fn acquiring_again_keeps_what_the_leases_watch_noted() {
        use crate::test_support::wayland::Compositor;

        let dir = crate::test_support::fresh_dir("engine-acquire-again");
        let niri = Compositor::start(&dir, include_str!("../tests/fixtures/us-de.xkb"));
        RuntimeDir::of(&niri.env(&dir)).unwrap().create().unwrap();
        let engine = Arc::new(without_niri(niri.env(&dir)));
        let session = crate::test_support::session(1);
        engine.grant(&session, "test/1", ready(None)).await.unwrap();
        assert_eq!(copied_and_read(&niri, &engine, &session).await, "copied");
        engine.grant(&session, "test/1", ready(None)).await.unwrap();
        let read = engine.clipboard(&session).await.unwrap();
        assert_eq!(serde_json::to_value(read).unwrap()["text"], "copied");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// `Engine::grant` for `session`, with nothing refused, in a task of its own.
    fn grant_apart(
        engine: &Arc<Engine>,
        session: &Session,
    ) -> tokio::task::JoinHandle<Result<(Holder, Option<u64>), ToolError>> {
        let (engine, session) = (Arc::clone(engine), session.clone());
        tokio::spawn(async move { engine.grant(&session, "test/1", ready(None)).await })
    }

    /// Grants `session` the lease once the desk lets it, within five seconds.
    async fn granted_soon(engine: &Arc<Engine>, session: &Session) {
        for _ in 0..500 {
            if engine.grant(session, "next/2", ready(None)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the lease wasn't granted within five seconds");
    }

    /// Copies "copied" as text, then returns the text `session` reads from the clipboard.
    async fn copied_and_read(
        niri: &crate::test_support::wayland::Compositor,
        engine: &Engine,
        session: &Session,
    ) -> String {
        let text = "text/plain;charset=utf-8".to_owned();
        niri.copy(Some(vec![(text, b"copied".to_vec())])).await;
        let read = engine.clipboard(session).await.unwrap();
        let read = serde_json::to_value(read).unwrap();
        read["text"].as_str().unwrap().to_owned()
    }

    /// A cleanup that runs until the returned sender is used or dropped.
    fn held_cleanup() -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<Result<(), ToolError>>,
    ) {
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        let cleanup = control::cleanup::spawn(control::cleanup::deadline(), async move {
            finished.await.ok();
            Ok(())
        });
        (finish, cleanup)
    }

    /// The idle engine doesn't exit while a dropped call's input cleanup runs: cut short,
    /// it leaves the marker for `recover`. Other tests' cleanups share the count, so only
    /// the engine's refusal to settle is asserted.
    #[tokio::test]
    async fn a_pending_cleanup_keeps_the_engine_from_settling() {
        let dir = crate::test_support::fresh_dir("engine-cleanup");
        let engine = without_niri(crate::test_support::niri_env(&dir));
        let (finish, cleanup) = held_cleanup();
        assert!(!engine.settled());
        finish.send(()).unwrap();
        cleanup.await.unwrap().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A standalone server whose client is gone ends only once a dropped call's input
    /// cleanup has. Other tests' cleanups share the count, so the end is awaited within
    /// the cleanups' limit.
    #[tokio::test]
    async fn a_standalone_server_ends_once_the_cleanup_its_client_left_has() {
        let dir = crate::test_support::fresh_dir("engine-alone-cleanup");
        let engine = Arc::new(without_niri(crate::test_support::niri_env(&dir)));
        let (finish, cleanup) = held_cleanup();
        let (client, ours) = tokio::io::duplex(64);
        drop(client);
        let (input, output) = tokio::io::split(ours);
        let session = crate::test_support::session(1);
        let mut served =
            tokio::spawn(async move { host::serve_alone(&engine, session, input, output).await });
        let early = tokio::time::timeout(Duration::from_millis(200), &mut served).await;
        assert!(early.is_err(), "the server ended before the cleanup");
        finish.send(()).unwrap();
        cleanup.await.unwrap().unwrap();
        tokio::time::timeout(control::cleanup::LIMIT, served)
            .await
            .expect("the server didn't end once the cleanup had")
            .unwrap()
            .ok();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An engine for `env` with no event stream and no accessibility bus.
    fn without_niri(env: Env) -> Engine {
        let events = Err(ToolError::new(ErrorName::NiriUnavailable, "no niri"));
        without_a11y(env, events)
    }

    fn without_a11y(env: Env, events: Result<EventStream, ToolError>) -> Engine {
        let absent = Presence {
            available: false,
            address: None,
            reason: Some("no session bus".to_owned()),
        };
        Engine::new(env, events, Audit::new(None), absent)
    }

    /// An empty desktop, as niri's event stream starts.
    const EMPTY_DESKTOP: &str = concat!(
        "{\"Ok\":\"Handled\"}\n",
        "{\"WorkspacesChanged\":{\"workspaces\":[]}}\n",
        "{\"WindowsChanged\":{\"windows\":[]}}\n",
        "{\"OverviewOpenedOrClosed\":{\"is_open\":false}}\n",
    );

    /// An engine whose event stream comes from a niri that serves nothing else: each
    /// connection gets an empty desktop and stays open until the returned `Notify` cuts
    /// it. The engine's other niri requests fail.
    async fn with_event_stream(dir: &std::path::Path) -> (Engine, Arc<tokio::sync::Notify>) {
        use tokio::io::AsyncWriteExt as _;

        let socket = dir.join("events.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let cut = Arc::new(tokio::sync::Notify::new());
        let cutting = Arc::clone(&cut);
        tokio::spawn(async move {
            while let Ok((mut connection, _)) = listener.accept().await {
                connection
                    .write_all(EMPTY_DESKTOP.as_bytes())
                    .await
                    .unwrap();
                cutting.notified().await;
            }
        });
        let events = Ok(EventStream::spawn(socket));
        let engine = without_a11y(crate::test_support::niri_env(dir), events);
        until(|| engine.connection() == Some(1)).await;
        (engine, cut)
    }

    impl Engine {
        fn connection(&self) -> Option<u64> {
            self.events.as_ref().ok().and_then(EventStream::connection)
        }
    }

    /// Polls `call` as far as its first wait, which must not be its end. A call waiting for
    /// the action mutex is queued there by then, behind the calls polled before it.
    async fn queue(call: &mut (impl Future + Unpin)) {
        tokio::select! {
            biased;
            _ = call => panic!("the call ended instead of waiting"),
            () = ready(()) => {}
        }
    }

    /// A zoom of the first pixels of the screenshot ref `id`.
    fn zoom_into(id: String) -> Zoom {
        let pixels = ImageRect {
            origin: crate::coords::ImagePx { x: 0, y: 0 },
            width: 10,
            height: 10,
        };
        Zoom { id, pixels }
    }

    /// Gives `owner` the lease and one screenshot ref, whose id it returns, and starts an
    /// observation of `owner` that holds the action mutex until the returned sender says.
    async fn owner_observing<'a>(
        engine: &'a Engine,
        owner: &'a Session,
    ) -> (
        String,
        impl Future<Output = Result<(), CallError>> + Unpin + 'a,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let desk = &engine.desk;
        desk.acquire(owner, "owner/1", ready(None), ready(None))
            .await
            .unwrap();
        let lease = desk.ref_lease(owner).unwrap();
        let id = desk.remember(lease, crate::test_support::shot()).unwrap();
        let (go, wait) = tokio::sync::oneshot::channel::<()>();
        let mut observing = Box::pin(desk.observe(owner, async {
            wait.await.ok();
            Ok(())
        }));
        queue(&mut observing).await;
        (id, observing, go)
    }

    /// A zoom waits behind the owner's running screenshot or action, and its ref is looked
    /// up once its turn comes: a release and a new lease queued ahead of it have dropped
    /// the ref by then.
    #[tokio::test]
    async fn a_queued_zoom_takes_its_ref_from_the_lease_of_its_turn() {
        let dir = crate::test_support::fresh_dir("engine-queued-zoom-lease");
        let (engine, _cut) = with_event_stream(&dir).await;
        let owner = crate::test_support::session(1);
        let (id, observing, go) = owner_observing(&engine, &owner).await;
        let mut release = Box::pin(engine.release(&owner, false));
        queue(&mut release).await;
        let mut acquire = Box::pin(engine.desk.acquire(
            &owner,
            "owner/1",
            ready(None),
            ready(None),
        ));
        queue(&mut acquire).await;
        let request = observe::Request::focused();
        let mut zoom = Box::pin(engine.screenshot(&owner, request, None, Some(zoom_into(id))));
        queue(&mut zoom).await;
        go.send(()).unwrap();
        observing.await.unwrap();
        assert!(release.await.released);
        acquire.await.unwrap();
        let Err(CallError::Tool(refused)) = zoom.await else {
            panic!("the zoom wasn't refused for its ref");
        };
        assert_eq!(refused.name, ErrorName::RefInvalid);
        assert!(
            refused.detail.contains("isn't a screenshot of this lease"),
            "{refused:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A ref checked once a queued zoom's turn comes is checked against the event stream
    /// of then: a reconnect while the zoom waited refuses it.
    #[tokio::test]
    async fn a_queued_zoom_checks_its_ref_against_the_event_stream_of_its_turn() {
        let dir = crate::test_support::fresh_dir("engine-queued-zoom-stream");
        let (engine, cut) = with_event_stream(&dir).await;
        let owner = crate::test_support::session(1);
        let (id, observing, go) = owner_observing(&engine, &owner).await;
        let request = observe::Request::focused();
        let mut zoom = Box::pin(engine.screenshot(&owner, request, None, Some(zoom_into(id))));
        queue(&mut zoom).await;
        cut.notify_one();
        until(|| engine.connection() == Some(2)).await;
        go.send(()).unwrap();
        observing.await.unwrap();
        let Err(CallError::Tool(refused)) = zoom.await else {
            panic!("the zoom wasn't refused for its ref");
        };
        assert_eq!(refused.name, ErrorName::RefInvalid);
        assert!(refused.detail.contains("reconnected"), "{refused:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Waits until `done`, for at most five seconds.
    async fn until(done: impl Fn() -> bool) {
        for _ in 0..500 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("still waiting after five seconds");
    }
}
