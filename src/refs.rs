//! Screenshot refs (plan §8): what each screenshot captured, kept in memory so a pointer
//! tool can map a pixel of that image back to the layout and a zoom can name pixels of it.
//! Element refs keep the accessible elements `elements` listed under the lease, so a
//! pointer tool can aim at one. Refs taken under the lease belong to that lease: taking or
//! giving up the lease drops them all. A session's screenshots without the lease belong to
//! that session's stretch without it, which taking the lease or the session's end closes,
//! and only its zooms take them. An id is never issued twice by one server, so a ref from
//! an earlier lease or stretch is simply unknown. Every id carries the server process's
//! tag, so neither is a ref from another process, such as a server that ran before this
//! one.

use std::collections::{BTreeMap, VecDeque};
use std::hash::BuildHasher as _;
use std::sync::OnceLock;
use std::time::Duration;

use niri_ipc::{LogicalOutput, Output};
use tokio::time::Instant;

use crate::a11y::ElementRef;
use crate::coords::{self, Capture, ImagePx, ImageRect, LayoutPt, ProtocolPt};
use crate::error::{ErrorName, ToolError};
use crate::niri::scene::Shifted;
use crate::observe::{Pin, Rect, Screenshot};
use crate::session::SessionId;

/// How many refs a lease keeps; older ones are dropped first.
const KEPT: usize = 64;
/// How many element refs a lease keeps; older ones are dropped first.
const KEPT_ELEMENTS: usize = 1000;
/// How long a ref stays usable (plan §6).
const MAX_AGE: Duration = Duration::from_secs(60);
/// How long niri may animate windows after it reports them moving, as for an element aim
/// (`input::pointer`).
const SETTLE: Duration = Duration::from_secs(1);

/// One screenshot, as a pointer tool needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shot {
    pub(crate) output: String,
    /// The output as niri described it at capture.
    pub(crate) geometry: LogicalOutput,
    pub(crate) motion_geometry: Option<LogicalOutput>,
    /// The captured rectangle in layout coordinates.
    pub(crate) captured: Rect,
    /// Image pixels per logical pixel.
    pub(crate) scale: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// When the capture started.
    pub(crate) taken: Instant,
    /// The event stream's connection at capture: a disconnect drops every ref.
    pub(crate) connection: u64,
    /// For a screenshot a settle wait returned as settled, when the capture it matched
    /// started: the screen didn't change from then on.
    pub(crate) steady_since: Option<Instant>,
}

impl Shot {
    /// What a pointer tool needs from `shot`, whose capture started at `taken` while the
    /// event stream was on `connection`.
    pub(crate) fn of(shot: &Screenshot, taken: Instant, connection: u64) -> Self {
        let metadata = &shot.metadata;
        Self {
            output: metadata.output.clone(),
            geometry: shot.geometry,
            motion_geometry: shot.motion_geometry,
            captured: metadata.captured,
            scale: metadata.scale,
            width: metadata.width,
            height: metadata.height,
            taken,
            connection,
            steady_since: None,
        }
    }
}

impl Shot {
    /// Where layout point `point` is now, as a `motion_absolute` on the captured output,
    /// provided the ref is fresh, the output is unchanged in `outputs`, the event stream
    /// is still on the capture's `connection`, and the point is inside the captured
    /// rectangle.
    pub(crate) fn aim_at(
        &self,
        point: LayoutPt,
        now: Instant,
        outputs: &BTreeMap<String, Output>,
        connection: Option<u64>,
    ) -> Result<ProtocolPt, ToolError> {
        let motion = self.check(now, outputs, connection)?;
        if !self.contains(point) {
            return Err(invalid(
                "out_of_bounds",
                &format!(
                    "({:.2}, {:.2}) is outside the screenshot's {:?}",
                    point.x, point.y, self.captured
                ),
            ));
        }
        self.encode(point, &motion)
    }

    /// Whether a pixel of the image still lands on what it shows, as far as niri can tell,
    /// when the windows on its output last moved as `shifted` says: not if they moved after
    /// the capture began, nor within `SETTLE` before it, while niri may still have been
    /// animating them, unless the screen was seen not changing after they moved.
    pub(crate) fn unmoved(&self, shifted: Option<&Shifted>) -> Result<(), ToolError> {
        let Some(Shifted { at, what }) = shifted else {
            return Ok(());
        };
        let detail = if *at > self.taken {
            format!(
                "{what} since the screenshot, so it may no longer show what a pixel lands on; take a new screenshot"
            )
        } else if self.taken.duration_since(*at) < SETTLE
            && self.steady_since.is_none_or(|steady| steady <= *at)
        {
            let before = self.taken.duration_since(*at).as_millis();
            format!(
                "{what} {before} ms before the screenshot, so it may show windows niri was still moving; take one that waits for the screen to settle, with `wait_for` with `until: screen_stable` and `screenshot: true`, or `screenshot: true` on the action"
            )
        } else {
            return Ok(());
        };
        Err(invalid("layout_changed", &detail))
    }

    /// Whether `point` lies in the captured rectangle.
    pub(crate) fn contains(&self, point: LayoutPt) -> bool {
        let Rect {
            x,
            y,
            width,
            height,
        } = self.captured;
        let (x, y) = (f64::from(x), f64::from(y));
        (x..x + f64::from(width)).contains(&point.x)
            && (y..y + f64::from(height)).contains(&point.y)
    }

    /// The layout point a pixel of this image targets, its centre, if the pixel is inside
    /// the image.
    pub(crate) fn layout_of(&self, pixel: ImagePx) -> Result<LayoutPt, ToolError> {
        if pixel.x >= self.width || pixel.y >= self.height {
            return Err(invalid(
                "out_of_bounds",
                &format!(
                    "pixel ({}, {}) is outside the {}x{} image",
                    pixel.x, pixel.y, self.width, self.height
                ),
            ));
        }
        let capture = Capture {
            origin: LayoutPt {
                x: f64::from(self.captured.x),
                y: f64::from(self.captured.y),
            },
            scale: self.scale,
        };
        Ok(coords::image_to_layout(pixel, capture))
    }

    /// The ref is fresh, the event stream is still on the capture's connection, and the
    /// output is unchanged: the pointer space to encode in.
    pub(crate) fn check(
        &self,
        now: Instant,
        outputs: &BTreeMap<String, Output>,
        connection: Option<u64>,
    ) -> Result<LogicalOutput, ToolError> {
        self.unchanged(now, outputs, connection)?;
        let motion = self.motion_geometry.ok_or_else(|| {
            invalid(
                "unknown_geometry",
                "the screenshot output has no usable physical mode; take a new screenshot",
            )
        })?;
        let now_motion = outputs.get(&self.output).and_then(coords::motion_geometry);
        if now_motion != Some(motion) {
            return Err(invalid(
                "output_changed",
                "the output's pointer space changed since capture; take a new screenshot",
            ));
        }
        Ok(motion)
    }

    /// The layout rectangle, in whole logical pixels, that covers `pixels` of this image,
    /// for a zoom into it, provided the ref is fresh, the event stream is still on the
    /// capture's connection, and `pixels` lies inside the image; with the output it lies on,
    /// which the capture checks is unchanged.
    pub(crate) fn zoom(
        &self,
        pixels: ImageRect,
        now: Instant,
        connection: Option<u64>,
    ) -> Result<(Rect, Pin), ToolError> {
        self.fresh(now, connection)?;
        let ImageRect {
            origin,
            width,
            height,
        } = pixels;
        let fits = |start: u32, extent: u32, image: u32| {
            extent > 0 && u64::from(start) + u64::from(extent) <= u64::from(image)
        };
        if !fits(origin.x, width, self.width) || !fits(origin.y, height, self.height) {
            return Err(invalid(
                "out_of_bounds",
                &format!(
                    "region {},{} {width}x{height} isn't inside the {}x{} image",
                    origin.x, origin.y, self.width, self.height
                ),
            ));
        }
        let (left, right) = logical_span(origin.x, width, self.scale, self.captured.width);
        let (top, bottom) = logical_span(origin.y, height, self.scale, self.captured.height);
        let region = Rect {
            x: self.captured.x.saturating_add_unsigned(left),
            y: self.captured.y.saturating_add_unsigned(top),
            width: right - left,
            height: bottom - top,
        };
        let pin = Pin {
            output: self.output.clone(),
            geometry: self.geometry,
        };
        Ok((region, pin))
    }

    /// The ref is fresh, the event stream is still on the capture's connection, and the
    /// output's layout is unchanged.
    fn unchanged(
        &self,
        now: Instant,
        outputs: &BTreeMap<String, Output>,
        connection: Option<u64>,
    ) -> Result<(), ToolError> {
        self.fresh(now, connection)?;
        let now_geometry = outputs.get(&self.output).and_then(|output| output.logical);
        if now_geometry != Some(self.geometry) {
            return Err(invalid(
                "output_changed",
                &format!(
                    "output {} was {:?} at the screenshot and is {now_geometry:?} now; take a new one",
                    self.output, self.geometry
                ),
            ));
        }
        Ok(())
    }

    /// The ref is fresh and the event stream is still on the capture's connection.
    fn fresh(&self, now: Instant, connection: Option<u64>) -> Result<(), ToolError> {
        let age = now.saturating_duration_since(self.taken);
        if age > MAX_AGE {
            return Err(invalid(
                "expired",
                &format!("the screenshot is {} s old; take a new one", age.as_secs()),
            ));
        }
        if connection != Some(self.connection) {
            return Err(invalid(
                "unknown_ref",
                "niri's event stream reconnected since the screenshot; take a new one",
            ));
        }
        Ok(())
    }

    fn encode(&self, point: LayoutPt, motion: &LogicalOutput) -> Result<ProtocolPt, ToolError> {
        coords::checked_encode(point, motion).map_err(|distance| {
            invalid(
                "out_of_bounds",
                &format!(
                    "({:.2}, {:.2}) maps {distance:.3} px off output {}",
                    point.x, point.y, self.output
                ),
            )
        })
    }
}

/// The whole logical pixels, as offsets from the captured rectangle's edge, that cover image
/// pixels `start` to `start + extent` at `scale`, within the `captured` logical pixels.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "both ends lie in 0..=captured, a u32, once the pixels are inside the image"
)]
fn logical_span(start: u32, extent: u32, scale: f64, captured: u32) -> (u32, u32) {
    let low = (f64::from(start) / scale).floor() as u32;
    let high = (f64::from(start + extent) / scale).ceil() as u32;
    (
        low.min(captured.saturating_sub(1)),
        high.clamp(low + 1, captured),
    )
}

/// `ref_invalid`, with the reason first.
pub(crate) fn invalid(reason: &str, detail: &str) -> ToolError {
    ToolError::new(ErrorName::RefInvalid, format!("{reason}: {detail}"))
}

/// Whether `id` has the form refs are issued in: `shot-` or `elem-`, a process's tag of
/// eight hex digits, `-` and the number, whichever process issued it.
pub(crate) fn well_formed(id: &str) -> bool {
    let Some(rest) = id
        .strip_prefix("shot-")
        .or_else(|| id.strip_prefix("elem-"))
    else {
        return false;
    };
    let Some((tag, number)) = rest.split_once('-') else {
        return false;
    };
    tag.len() == 8
        && tag.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        && issued_number(number).is_some()
}

/// The number at the end of an id, spelled as ids are issued: decimal digits without a
/// sign or leading zeros, so `+1` or `01` names no ref.
fn issued_number(digits: &str) -> Option<u64> {
    digits
        .parse()
        .ok()
        .filter(|number: &u64| number.to_string() == digits)
}

/// The refs of the current lease, and the screenshot refs of sessions without it, which
/// only a zoom takes.
#[derive(Debug)]
pub(crate) struct Refs {
    /// In every id, between the kind and the number.
    tag: String,
    /// Leases seen, counted from 1, and refs issued: ids are never reused.
    leases: u64,
    issued: u64,
    /// The lease the kept refs belong to, while one is held.
    lease: Option<u64>,
    shots: VecDeque<(u64, Shot)>,
    /// Each session's screenshots taken without the lease, numbered with the lease's.
    views: BTreeMap<SessionId, Views>,
    elements_issued: u64,
    elements: VecDeque<(u64, ElementRef)>,
}

/// A session's screenshots of its current stretch without the lease, and which stretch that
/// is: each time the session takes the lease, a new one starts.
#[derive(Debug, Default)]
struct Views {
    stretch: u64,
    shots: VecDeque<(u64, Shot)>,
}

impl Default for Refs {
    /// Refs whose ids carry this process's tag.
    fn default() -> Self {
        Self::new(process_tag().to_owned())
    }
}

/// Eight hex digits, random for each process.
fn process_tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| {
        // The standard library seeds its hash keys from the system's randomness.
        let random = std::hash::RandomState::new().hash_one(std::process::id());
        format!("{:08x}", random & 0xFFFF_FFFF)
    })
}

impl Refs {
    /// No refs yet, with `tag` in every id.
    pub(crate) const fn new(tag: String) -> Self {
        Self {
            tag,
            leases: 0,
            issued: 0,
            lease: None,
            shots: VecDeque::new(),
            views: BTreeMap::new(),
            elements_issued: 0,
            elements: VecDeque::new(),
        }
    }

    /// The number in `id`, if it is an id of `kind` with this tag.
    fn number(&self, id: &str, kind: &str) -> Option<u64> {
        let rest = id.strip_prefix(kind)?.strip_prefix('-')?;
        issued_number(rest.strip_prefix(self.tag.as_str())?.strip_prefix('-')?)
    }

    /// A new lease, for `owner`: earlier refs of the lease and `owner`'s refs from before
    /// it are dropped, and its next stretch without the lease begins.
    pub(crate) fn start(&mut self, owner: SessionId) {
        let views = self.views.entry(owner).or_default();
        views.stretch += 1;
        views.shots.clear();
        self.leases += 1;
        self.lease = Some(self.leases);
        self.shots.clear();
        self.elements.clear();
    }

    /// The lease is gone, and with it its refs.
    pub(crate) fn end(&mut self) {
        self.lease = None;
        self.shots.clear();
        self.elements.clear();
    }

    /// The lease the next `start` begins.
    pub(crate) const fn next(&self) -> u64 {
        self.leases + 1
    }

    /// The lease a screenshot starting now would belong to.
    pub(crate) const fn lease(&self) -> Option<u64> {
        self.lease
    }

    /// Keeps `shot` and returns its id, if `lease` is still the current one.
    pub(crate) fn insert(&mut self, lease: u64, shot: Shot) -> Option<String> {
        if self.lease != Some(lease) {
            return None;
        }
        self.issued += 1;
        if self.shots.len() == KEPT {
            self.shots.pop_front();
        }
        self.shots.push_back((self.issued, shot));
        Some(format!("shot-{}-{}", self.tag, self.issued))
    }

    /// `session`'s current stretch without the lease, which a screenshot starting now would
    /// belong to.
    pub(crate) fn stretch(&self, session: SessionId) -> u64 {
        self.views.get(&session).map_or(0, |views| views.stretch)
    }

    /// Keeps `shot`, which `session` took without the lease, and returns its id, if
    /// `stretch` is still the session's current one.
    pub(crate) fn insert_view(
        &mut self,
        session: SessionId,
        stretch: u64,
        shot: Shot,
    ) -> Option<String> {
        let views = self.views.entry(session).or_default();
        if views.stretch != stretch {
            return None;
        }
        self.issued += 1;
        if views.shots.len() == KEPT {
            views.shots.pop_front();
        }
        views.shots.push_back((self.issued, shot));
        Some(format!("shot-{}-{}", self.tag, self.issued))
    }

    /// Drops the refs `session` took without the lease, once it has ended.
    pub(crate) fn forget(&mut self, session: SessionId) {
        self.views.remove(&session);
    }

    /// The ref named `id` that `session` took without the lease.
    pub(crate) fn view(&self, session: SessionId, id: &str) -> Result<Shot, ToolError> {
        let wanted = self.number(id, "shot");
        self.views
            .get(&session)
            .and_then(|views| {
                views
                    .shots
                    .iter()
                    .find(|(issued, _)| Some(*issued) == wanted)
            })
            .map(|(_, shot)| shot.clone())
            .ok_or_else(|| {
                invalid(
                    "unknown_ref",
                    &format!("{id:?} isn't a screenshot this session still keeps; take a new one"),
                )
            })
    }

    /// Keeps `element` and returns its id, `elem-<tag>-N`, if `lease` is still the current
    /// one.
    pub(crate) fn insert_element(&mut self, lease: u64, element: ElementRef) -> Option<String> {
        if self.lease != Some(lease) {
            return None;
        }
        self.elements_issued += 1;
        if self.elements.len() == KEPT_ELEMENTS {
            self.elements.pop_front();
        }
        self.elements.push_back((self.elements_issued, element));
        Some(format!("elem-{}-{}", self.tag, self.elements_issued))
    }

    /// The element ref named `id`, if the current lease kept it.
    pub(crate) fn element(&self, id: &str) -> Result<ElementRef, ToolError> {
        let wanted = self.number(id, "elem");
        self.elements
            .iter()
            .find(|(issued, _)| Some(*issued) == wanted)
            .map(|(_, element)| element.clone())
            .ok_or_else(|| {
                ToolError::new(
                    ErrorName::ElementStale,
                    format!(
                        "{id:?} isn't an element listed under this lease; call elements after acquire_desktop"
                    ),
                )
            })
    }

    /// The ref named `id`, if the current lease kept it.
    pub(crate) fn get(&self, id: &str) -> Result<Shot, ToolError> {
        let wanted = self.number(id, "shot");
        self.shots
            .iter()
            .find(|(issued, _)| Some(*issued) == wanted)
            .map(|(_, shot)| shot.clone())
            .ok_or_else(|| {
                invalid(
                    "unknown_ref",
                    &format!(
                        "{id:?} isn't a screenshot of this lease; take a screenshot after acquire_desktop"
                    ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::test_support::shot;

    fn outputs(geometry: LogicalOutput) -> BTreeMap<String, Output> {
        let mode = crate::test_support::output_mode::from_logical(
            geometry.width,
            geometry.height,
            geometry.scale,
        );
        let output = serde_json::from_value(serde_json::json!({
            "name": "winit", "make": "", "model": "", "serial": null, "physical_size": null,
            "modes": [mode], "current_mode": 0, "is_custom_mode": false,
            "vrr_supported": false, "vrr_enabled": false, "logical": geometry
        }))
        .unwrap();
        BTreeMap::from([("winit".to_owned(), output)])
    }

    /// A pixel of `shot` aimed as the pointer tools aim it.
    fn aim(
        shot: &Shot,
        pixel: ImagePx,
        now: Instant,
        outputs: &BTreeMap<String, Output>,
        connection: Option<u64>,
    ) -> Result<ProtocolPt, ToolError> {
        shot.check(now, outputs, connection)?;
        shot.aim_at(shot.layout_of(pixel)?, now, outputs, connection)
    }

    fn reason(result: Result<ProtocolPt, ToolError>) -> String {
        let error = result.unwrap_err();
        assert_eq!(error.name, ErrorName::RefInvalid);
        error.detail.split(':').next().unwrap().to_owned()
    }

    #[tokio::test(start_paused = true)]
    async fn pixels_stand_unless_the_windows_moved_after_or_just_before_the_capture() {
        tokio::time::advance(Duration::from_secs(5)).await;
        let mut shot = shot();
        let taken = shot.taken;
        let shifted = |ms: i64| Shifted {
            at: if ms < 0 {
                taken - Duration::from_millis(ms.unsigned_abs())
            } else {
                taken + Duration::from_millis(ms.unsigned_abs())
            },
            what: "niri moved or resized window 7".to_owned(),
        };
        let refused = |aimed: &Shot, ms| {
            aimed
                .unmoved(Some(&shifted(ms)))
                .err()
                .map(|error| error.detail)
        };
        assert_eq!(shot.unmoved(None), Ok(()));
        // Long enough before the capture for niri to have finished moving it.
        assert_eq!(refused(&shot, -1500), None);
        let moving = refused(&shot, -300).unwrap();
        assert!(moving.contains("300 ms before the screenshot"), "{moving}");
        // Another plain screenshot right away would be refused the same way.
        let settled = "`wait_for` with `until: screen_stable` and `screenshot: true`";
        assert!(moving.contains(settled), "{moving}");
        let since = refused(&shot, 20).unwrap();
        assert!(since.contains("window 7 since the screenshot"), "{since}");
        // The screen was seen not changing from after the move on.
        shot.steady_since = Some(taken - Duration::from_millis(100));
        assert_eq!(refused(&shot, -300), None);
        assert!(refused(&shot, -50).is_some());
        assert!(refused(&shot, 20).is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_or_changed_physical_mode_never_reuses_a_pointer_mapping() {
        let mut shot = shot();
        let mut now = outputs(shot.geometry);
        let centre = ImagePx { x: 720, y: 540 };
        now.get_mut("winit")
            .unwrap()
            .modes
            .first_mut()
            .unwrap()
            .width += 1;
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(1))),
            "output_changed"
        );
        now.get_mut("winit").unwrap().modes.clear();
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(1))),
            "output_changed"
        );
        shot.motion_geometry = None;
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(1))),
            "unknown_geometry"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn fractional_output_motion_uses_niris_ceiled_space_not_its_truncated_ipc_size() {
        let mut shot = shot();
        shot.geometry.width = 853;
        shot.geometry.height = 480;
        shot.captured.width = 853;
        shot.captured.height = 480;
        shot.width = 1279;
        shot.height = 720;
        let mut now = outputs(shot.geometry);
        let output = now.get_mut("winit").unwrap();
        output.modes = vec![niri_ipc::Mode {
            width: 1280,
            height: 720,
            refresh_rate: 60000,
            is_preferred: true,
        }];
        output.current_mode = Some(0);
        shot.motion_geometry = coords::motion_geometry(output);
        let pixel = ImagePx { x: 600, y: 300 };
        let request = aim(&shot, pixel, Instant::now(), &now, Some(1)).unwrap();
        // niri 26.04 uses Smithay's ceiled output space: ceil(1280 / 1.5) = 854.
        let landed_x = f64::from(request.x) * 854.0 / f64::from(request.x_extent);
        let target_x = 600.5 / 1.5;
        assert!(
            (landed_x - target_x).abs() <= 0.002,
            "landed {landed_x}, target {target_x}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_ref_aims_only_while_fresh_unchanged_and_inside_its_image() {
        let shot = shot();
        let now = outputs(shot.geometry);
        let centre = ImagePx { x: 720, y: 540 };
        // The image's centre at scale 1.5 is the output's, mirrored vertically by
        // Flipped180 into the untransformed space: (480, 360) either way.
        assert_eq!(
            aim(&shot, centre, Instant::now(), &now, Some(1)),
            Ok(ProtocolPt {
                x: 480_333,
                y: 359_667,
                x_extent: 960_000,
                y_extent: 720_000
            })
        );
        let later = Instant::now() + MAX_AGE + Duration::from_secs(1);
        assert_eq!(reason(aim(&shot, centre, later, &now, Some(1))), "expired");
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &now, Some(2))),
            "unknown_ref"
        );
        let moved = outputs(LogicalOutput {
            scale: 1.0,
            ..shot.geometry
        });
        assert_eq!(
            reason(aim(&shot, centre, Instant::now(), &moved, Some(1))),
            "output_changed"
        );
        assert_eq!(
            reason(aim(
                &shot,
                centre,
                Instant::now(),
                &BTreeMap::new(),
                Some(1)
            )),
            "output_changed"
        );
        for pixel in [ImagePx { x: 1440, y: 0 }, ImagePx { x: 0, y: 1080 }] {
            assert_eq!(
                reason(aim(&shot, pixel, Instant::now(), &now, Some(1))),
                "out_of_bounds"
            );
        }
        assert!(
            aim(
                &shot,
                ImagePx { x: 1439, y: 1079 },
                Instant::now(),
                &now,
                Some(1)
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn an_id_from_another_process_names_nothing_here() {
        let mut earlier = Refs::new("0a1b2c3d".to_owned());
        let mut now = Refs::new("4e5f6a7b".to_owned());
        earlier.start(SessionId(1));
        now.start(SessionId(1));
        let old = earlier.insert(earlier.lease().unwrap(), shot()).unwrap();
        let new = now.insert(now.lease().unwrap(), shot()).unwrap();
        assert_ne!(old, new);
        assert!(now.get(&new).is_ok());
        let error = now.get(&old).unwrap_err();
        assert!(error.detail.starts_with("unknown_ref"), "{}", error.detail);
    }

    /// Whether `rect` holds the first and last of `pixels` of `shot`'s image.
    fn covers(shot: &Shot, pixels: ImageRect, rect: Rect) {
        let zoomed = Shot {
            captured: rect,
            ..shot.clone()
        };
        let ImageRect { origin, .. } = pixels;
        let last = ImagePx {
            x: origin.x + pixels.width - 1,
            y: origin.y + pixels.height - 1,
        };
        for pixel in [origin, last] {
            let point = shot.layout_of(pixel).unwrap();
            assert!(
                zoomed.contains(point),
                "{pixel:?} of {pixels:?} not in {rect:?}"
            );
        }
    }

    /// A zoom must capture every pixel the agent pointed at, at the screenshot's own place
    /// in the layout, whatever the scale rounds to, and nothing outside the screenshot.
    #[tokio::test(start_paused = true)]
    async fn a_zoom_covers_the_pixels_asked_for_and_stays_inside_the_screenshot() {
        let mut shot = shot();
        shot.captured = Rect {
            x: 100,
            y: 40,
            width: 301,
            height: 201,
        };
        (shot.width, shot.height) = (451, 301);
        let regions = [
            (0, 0, 1, 1),
            (1, 1, 1, 1),
            (150, 31, 30, 15),
            (449, 299, 2, 2),
        ];
        for (x, y, width, height) in regions {
            let pixels = ImageRect {
                origin: ImagePx { x, y },
                width,
                height,
            };
            let (rect, pin) = shot.zoom(pixels, Instant::now(), Some(1)).unwrap();
            assert_eq!(
                (pin.output.as_str(), pin.geometry),
                ("winit", shot.geometry)
            );
            covers(&shot, pixels, rect);
            assert!(rect.x >= 100 && rect.y >= 40, "{rect:?}");
            assert!(
                rect.x + i32::try_from(rect.width).unwrap() <= 401,
                "{rect:?}"
            );
            assert!(
                rect.y + i32::try_from(rect.height).unwrap() <= 241,
                "{rect:?}"
            );
        }
        let at = |x, y| ImageRect {
            origin: ImagePx { x, y },
            width: 2,
            height: 2,
        };
        assert_eq!(
            shot.zoom(at(150, 30), Instant::now(), Some(1))
                .map(|(rect, _)| rect),
            Ok(Rect {
                x: 200,
                y: 60,
                width: 2,
                height: 2,
            })
        );
        let outside = shot.zoom(at(450, 0), Instant::now(), Some(1));
        assert!(outside.unwrap_err().detail.starts_with("out_of_bounds"));
    }

    /// A screenshot taken without the lease is only for zooming, by the session that took
    /// it, until that session takes the lease; one that started before the session took the
    /// lease, even a lease since given up, gets no ref.
    #[tokio::test]
    async fn a_ref_taken_without_the_lease_is_its_sessions_own_until_it_takes_the_lease() {
        let (me, other) = (SessionId(1), SessionId(2));
        let mut refs = Refs::new("t".to_owned());
        let started = refs.stretch(me);
        let mine = refs.insert_view(me, started, shot()).unwrap();
        assert!(refs.view(me, &mine).is_ok());
        assert!(refs.view(other, &mine).is_err());
        // Pointer tools look only at the lease's refs.
        refs.start(other);
        assert!(refs.get(&mine).is_err());
        assert!(refs.view(me, &mine).is_ok());
        refs.end();
        refs.start(me);
        refs.end();
        let error = refs.view(me, &mine).unwrap_err();
        assert!(error.detail.starts_with("unknown_ref"), "{}", error.detail);
        assert_eq!(refs.insert_view(me, started, shot()), None);
        assert!(refs.insert_view(me, refs.stretch(me), shot()).is_some());
        let theirs = refs
            .insert_view(other, refs.stretch(other), shot())
            .unwrap();
        refs.forget(other);
        assert!(refs.view(other, &theirs).is_err());
    }

    /// Found by a property test: `parse` takes a sign and leading zeros, so these named
    /// ref 1 while the audit log called them `invalid_ref`.
    #[test]
    fn an_id_names_a_ref_only_as_it_was_issued() {
        let mut refs = Refs::new("0123abcd".to_owned());
        refs.start(SessionId(1));
        let lease = refs.lease().unwrap();
        assert_eq!(
            refs.insert(lease, shot()).as_deref(),
            Some("shot-0123abcd-1")
        );
        assert!(refs.get("shot-0123abcd-1").is_ok());
        for misspelled in ["shot-0123abcd-+1", "shot-0123abcd-01"] {
            let error = refs.get(misspelled).unwrap_err();
            assert!(error.detail.starts_with("unknown_ref"), "{misspelled}");
            assert!(!well_formed(misspelled), "{misspelled}");
        }
    }

    #[test]
    fn element_refs_belong_to_one_lease_and_keep_the_latest_thousand() {
        let element = || ElementRef {
            bus: ":1.7".to_owned(),
            path: "/org/a11y/atspi/accessible/9".to_owned(),
            frame: "/org/a11y/atspi/accessible/1".to_owned(),
            page: None,
            kept: crate::a11y::model::Kept {
                role: 43,
                window: 3,
                pid: 4711,
                actions: Vec::new(),
                lineage: Vec::new(),
                row: crate::a11y::model::Row::Lineage,
            },
        };
        let stale = |refs: &Refs, id: &str| refs.element(id).unwrap_err().name;
        let mut refs = Refs::new("t".to_owned());
        assert_eq!(refs.insert_element(1, element()), None);
        refs.start(SessionId(1));
        let first = refs.lease().unwrap();
        assert_eq!(
            refs.insert_element(first, element()).as_deref(),
            Some("elem-t-1")
        );
        assert_eq!(refs.element("elem-t-1"), Ok(element()));
        refs.start(SessionId(1));
        let second = refs.lease().unwrap();
        assert_eq!(stale(&refs, "elem-t-1"), ErrorName::ElementStale);
        assert_eq!(refs.insert_element(first, element()), None);
        for _ in 0..=KEPT_ELEMENTS {
            refs.insert_element(second, element());
        }
        assert_eq!(stale(&refs, "elem-t-2"), ErrorName::ElementStale);
        assert!(refs.element("elem-t-3").is_ok());
        assert_eq!(stale(&refs, "shot-t-3"), ErrorName::ElementStale);
    }

    #[tokio::test]
    async fn refs_belong_to_one_lease_and_ids_are_never_reused() {
        let mut refs = Refs::new("t".to_owned());
        assert_eq!(refs.lease(), None);
        assert_eq!(refs.insert(1, shot()), None);
        refs.start(SessionId(1));
        let first = refs.lease().unwrap();
        assert_eq!(refs.insert(first, shot()).as_deref(), Some("shot-t-1"));
        refs.end();
        // A capture that started under the old lease isn't kept.
        assert_eq!(refs.insert(first, shot()), None);
        refs.start(SessionId(1));
        let second = refs.lease().unwrap();
        assert_ne!(first, second);
        assert_eq!(refs.insert(first, shot()), None);
        assert_eq!(refs.insert(second, shot()).as_deref(), Some("shot-t-2"));
        assert_eq!(refs.shots.len(), 1);
        assert!(refs.get("shot-t-2").is_ok());
        for unknown in ["shot-t-1", "shot-t-9", "shot-2", "2", "shot-t-x"] {
            assert_eq!(
                reason(refs.get(unknown).map(|_| ProtocolPt {
                    x: 0,
                    y: 0,
                    x_extent: 1,
                    y_extent: 1
                })),
                "unknown_ref"
            );
        }
        for _ in 0..KEPT {
            refs.insert(second, shot());
        }
        assert_eq!(refs.shots.len(), KEPT);
        assert_eq!(refs.shots.front().map(|(id, _)| *id), Some(3));
    }

    fn element(pid: i32) -> ElementRef {
        ElementRef {
            bus: ":1.7".to_owned(),
            path: "/org/a11y/atspi/accessible/9".to_owned(),
            frame: "/org/a11y/atspi/accessible/1".to_owned(),
            page: None,
            kept: crate::a11y::model::Kept {
                role: 43,
                window: 3,
                pid,
                actions: Vec::new(),
                lineage: Vec::new(),
                row: crate::a11y::model::Row::Lineage,
            },
        }
    }

    /// Ids close to the issued ones, as an agent could misspell them, and any text.
    fn asked() -> impl Strategy<Value = String> {
        prop_oneof![
            "(shot|elem)-(0123abcd|0123ABCD|[0-9a-f]{8})-[+0 ]?[0-9]{1,2} ?",
            any::<String>(),
        ]
    }

    proptest! {
        #![proptest_config(crate::test_support::properties(512))]

        /// Every ref the server issues has the form the audit log keeps as it is and names
        /// what it was issued for, and no other spelling names anything: an id resolves
        /// only as it was issued.
        #[test]
        fn only_the_ids_issued_name_a_ref(count in 1..=32_i32, asked in asked()) {
            let (holder, viewer) = (SessionId(1), SessionId(2));
            let mut refs = Refs::new("0123abcd".to_owned());
            refs.start(holder);
            let lease = refs.lease().unwrap();
            let stretch = refs.stretch(viewer);
            let mut issued = (Vec::new(), Vec::new(), Vec::new());
            for n in 0..count {
                let mut taken = shot();
                taken.connection = n.unsigned_abs().into();
                let shot_id = refs.insert(lease, taken.clone()).unwrap();
                prop_assert!(well_formed(&shot_id), "{shot_id}");
                prop_assert_eq!(refs.get(&shot_id), Ok(taken.clone()));
                issued.0.push(shot_id);
                let view_id = refs.insert_view(viewer, stretch, taken.clone()).unwrap();
                prop_assert!(well_formed(&view_id), "{view_id}");
                prop_assert_eq!(refs.view(viewer, &view_id), Ok(taken));
                issued.1.push(view_id);
                let element_id = refs.insert_element(lease, element(n)).unwrap();
                prop_assert!(well_formed(&element_id), "{element_id}");
                prop_assert_eq!(refs.element(&element_id), Ok(element(n)));
                issued.2.push(element_id);
            }
            prop_assert_eq!(refs.get(&asked).is_ok(), issued.0.contains(&asked));
            prop_assert_eq!(refs.view(viewer, &asked).is_ok(), issued.1.contains(&asked));
            prop_assert_eq!(refs.element(&asked).is_ok(), issued.2.contains(&asked));
        }
    }

    /// grim's image size for `logical` pixels at `scale`.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "grim truncates the same way, `int common_width = geometry->width * scale`"
    )]
    fn grim_pixels(logical: u32, scale: f64) -> u32 {
        (f64::from(logical) * scale) as u32
    }

    /// The pixel `fraction` of the way across `size` pixels.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a fraction below 1 of a u32 size, floored"
    )]
    fn across(size: u32, fraction: f64) -> u32 {
        (f64::from(size) * fraction).floor() as u32
    }

    /// Outputs as niri reports them: a physical mode, a scale in niri's 1/120 steps and a
    /// transform, with the logical size truncated, anywhere in the layout.
    fn any_output() -> impl Strategy<Value = (LogicalOutput, BTreeMap<String, Output>)> {
        let transforms = prop::sample::select(vec![
            niri_ipc::Transform::Normal,
            niri_ipc::Transform::_90,
            niri_ipc::Transform::_180,
            niri_ipc::Transform::_270,
            niri_ipc::Transform::Flipped,
            niri_ipc::Transform::Flipped90,
            niri_ipc::Transform::Flipped180,
            niri_ipc::Transform::Flipped270,
        ]);
        (
            (1..=7680_u32, 1..=4320_u32),
            60..=480_u32,
            transforms,
            (-100_000..=100_000_i32, -100_000..=100_000_i32),
        )
            .prop_filter_map(
                "an output smaller than a logical pixel",
                |((width, height), steps, transform, (x, y))| {
                    let scale = f64::from(steps) / 120.0;
                    let quarter = matches!(
                        transform,
                        niri_ipc::Transform::_90
                            | niri_ipc::Transform::_270
                            | niri_ipc::Transform::Flipped90
                            | niri_ipc::Transform::Flipped270
                    );
                    let (shown_width, shown_height) = if quarter {
                        (height, width)
                    } else {
                        (width, height)
                    };
                    let geometry = LogicalOutput {
                        x,
                        y,
                        width: grim_pixels(shown_width, 1.0 / scale),
                        height: grim_pixels(shown_height, 1.0 / scale),
                        scale,
                        transform,
                    };
                    let output = serde_json::from_value(serde_json::json!({
                        "name": "winit", "make": "", "model": "", "serial": null,
                        "physical_size": null, "current_mode": 0, "is_custom_mode": false,
                        "modes": [{"width": width, "height": height, "refresh_rate": 60000,
                                   "is_preferred": true}],
                        "vrr_supported": false, "vrr_enabled": false, "logical": geometry
                    }))
                    .ok()?;
                    (geometry.width > 0 && geometry.height > 0)
                        .then(|| (geometry, BTreeMap::from([("winit".to_owned(), output)])))
                },
            )
    }

    proptest! {
        #![proptest_config(crate::test_support::properties(512))]

        /// Every pixel of a screenshot of a whole output, at its own scale or one lowered
        /// for `max_width`, aims the pointer: an agent never gets `out_of_bounds` for a
        /// pixel it can see.
        #[test]
        fn every_pixel_of_an_output_screenshot_aims(
            (geometry, outputs) in any_output(),
            lowered in prop_oneof![Just(1.0), 0.05..1.0_f64],
            (fx, fy) in (0.0..1.0_f64, 0.0..1.0_f64),
        ) {
            let scale = geometry.scale * lowered;
            let (width, height) = (
                grim_pixels(geometry.width, scale),
                grim_pixels(geometry.height, scale),
            );
            prop_assume!(width > 0 && height > 0);
            let output = outputs.get("winit").unwrap();
            let shot = Shot {
                geometry,
                motion_geometry: coords::motion_geometry(output),
                captured: Rect {
                    x: geometry.x,
                    y: geometry.y,
                    width: geometry.width,
                    height: geometry.height,
                },
                scale,
                width,
                height,
                ..shot()
            };
            let now = Instant::now();
            for pixel in [
                ImagePx { x: across(width, fx), y: across(height, fy) },
                ImagePx { x: 0, y: 0 },
                ImagePx { x: width - 1, y: height - 1 },
                ImagePx { x: 0, y: height - 1 },
                ImagePx { x: width - 1, y: 0 },
            ] {
                let aimed = aim(&shot, pixel, now, &outputs, Some(shot.connection));
                prop_assert!(aimed.is_ok(), "{pixel:?} of {width}x{height}: {aimed:?}");
            }
        }

        /// A zoom into any region inside the image covers every pixel asked for and stays
        /// inside the screenshot; any other region is `out_of_bounds`, never a panic.
        #[test]
        fn a_zoom_covers_its_region_or_refuses_it(
            (x, y, width, height) in (-100_000..=100_000_i32, -100_000..=100_000_i32, 1..=4000_u32, 1..=4000_u32),
            scale in 0.05..4.0_f64,
            region in (0.0..1.2_f64, 0.0..1.2_f64, 0.0..1.2_f64, 0.0..1.2_f64),
        ) {
            let captured = Rect { x, y, width, height };
            let image = (grim_pixels(width, scale), grim_pixels(height, scale));
            prop_assume!(image.0 > 0 && image.1 > 0);
            let shot = Shot { captured, scale, width: image.0, height: image.1, ..shot() };
            let pixels = ImageRect {
                origin: ImagePx { x: across(image.0, region.0), y: across(image.1, region.1) },
                width: across(image.0, region.2),
                height: across(image.1, region.3),
            };
            let inside = |start: u32, extent: u32, size: u32| {
                extent > 0 && u64::from(start) + u64::from(extent) <= u64::from(size)
            };
            let zoomed = shot.zoom(pixels, Instant::now(), Some(shot.connection));
            if inside(pixels.origin.x, pixels.width, image.0)
                && inside(pixels.origin.y, pixels.height, image.1)
            {
                let (rect, _) = zoomed.unwrap();
                prop_assert!(rect.width > 0 && rect.height > 0, "{rect:?}");
                prop_assert!(rect.x >= x && rect.y >= y, "{rect:?} in {captured:?}");
                prop_assert!(
                    i64::from(rect.x) + i64::from(rect.width) <= i64::from(x) + i64::from(width)
                        && i64::from(rect.y) + i64::from(rect.height)
                            <= i64::from(y) + i64::from(height),
                    "{rect:?} in {captured:?}"
                );
                covers(&shot, pixels, rect);
            } else {
                let error = zoomed.unwrap_err();
                prop_assert!(error.detail.starts_with("out_of_bounds"), "{}", error.detail);
            }
        }
    }
}
