//! The notification `notify_on_lease` puts on the desktop when an agent is granted the
//! lease, so the user sees that one drives the desktop without the Noctalia plugin. It is
//! sent by `notify-send` through the session's notification server, at most once per agent
//! every `INTERVAL`, so an agent that takes the lease again and again doesn't flood the
//! desktop. Its failure never fails the grant: `status.lease_notice_error` keeps it.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

use crate::runner::{self, Place, Search};
use crate::session::Label;

/// The least time between two notifications for one agent.
pub(crate) const INTERVAL: Duration = Duration::from_mins(5);
/// `notify-send` returns once the notification server took the notification.
const DEADLINE: Duration = Duration::from_secs(2);
/// `notify-send` prints nothing without `--wait` or actions.
const MAX_STDOUT: u64 = 1024;

/// When each agent, by its label, was last notified about, and the last failure.
#[derive(Debug, Default)]
pub(crate) struct Notices {
    sent: Mutex<HashMap<String, Instant>>,
    last_error: Mutex<Option<String>>,
}

impl Notices {
    /// Whether `label` is due a notification at `now`; if it is, it counts as sent then.
    /// Agents not notified within `INTERVAL` are forgotten.
    pub(crate) fn due(&self, label: &Label, now: Instant) -> bool {
        let mut sent = self.sent.lock().unwrap_or_else(PoisonError::into_inner);
        sent.retain(|_, at| now.duration_since(*at) < INTERVAL);
        let key = label.to_string();
        if sent.contains_key(&key) {
            return false;
        }
        sent.insert(key, now);
        true
    }

    /// Why the last notification couldn't be sent, if it couldn't.
    pub(crate) fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Tells the user that the client `label`, working in `project`, took the lease, with
    /// `notify-send` found in `place`, within `DEADLINE`.
    pub(crate) async fn send(&self, place: Place<'_>, label: &Label, project: Option<&str>) {
        let ran = runner::run(
            "notify-send",
            Search::Client(place),
            &args(label, project),
            DEADLINE,
            MAX_STDOUT,
        )
        .await;
        let error = match ran {
            Ok(done) if done.status.success() => None,
            Ok(done) => Some(done.failure("notify-send").detail),
            Err(error) => Some(error.detail),
        };
        *self
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = error;
    }
}

/// `notify-send`'s arguments. The label holds no markup or line break (`Label`). The
/// project, a directory's name, may, and notification servers may read the body as
/// markup, so it is escaped.
fn args(label: &Label, project: Option<&str>) -> Vec<String> {
    let agent = project.map_or_else(
        || label.client().to_owned(),
        |project| format!("{} in {}", label.client(), escape(project)),
    );
    let body = format!(
        "{agent} (process {}) took the niri-computer-use lease. It can click, type and change windows until it releases the lease or you press the stop key.",
        label.pid()
    );
    vec![
        "--app-name=niri-computer-use".to_owned(),
        "--urgency=normal".to_owned(),
        "An agent is using your desktop".to_owned(),
        body,
    ]
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

    /// A directory holding a `notify-send` that runs `body`.
    fn notify_send(name: &str, body: &str) -> PathBuf {
        let bin = crate::test_support::fresh_dir(name);
        crate::test_support::script(&bin.join("notify-send"), body);
        bin
    }

    fn place(bin: &Path) -> Place<'_> {
        Place {
            path: Some(bin.as_os_str()),
            cwd: None,
        }
    }

    fn label(name: &str) -> Label {
        crate::test_support::session(1).label(name)
    }

    #[test]
    fn notifies_each_agent_at_most_once_per_interval() {
        let notices = Notices::default();
        let (claude, pi) = (label("claude-code"), label("pi"));
        let start = Instant::now();
        assert!(notices.due(&claude, start));
        // Taking the lease again and again, alternating with another agent.
        for seconds in [5, 60, 299] {
            let now = start + Duration::from_secs(seconds);
            assert!(!notices.due(&claude, now), "{seconds} s");
            assert_eq!(notices.due(&pi, now), seconds == 5, "{seconds} s");
        }
        assert!(notices.due(&claude, start + INTERVAL));
    }

    #[tokio::test]
    async fn names_the_agent_and_its_project() {
        let bin = notify_send("notice-sent", "printf '%s\\n' \"$@\" > \"${0%/*}/args\"");
        let notices = Notices::default();
        notices
            .send(place(&bin), &label("claude-code"), Some("shop<1>"))
            .await;
        assert_eq!(notices.last_error(), None);
        let sent = std::fs::read_to_string(bin.join("args")).unwrap();
        let lines: Vec<&str> = sent.lines().collect();
        assert_eq!(lines[2], "An agent is using your desktop");
        assert!(
            lines[3].starts_with("claude-code in shop&lt;1&gt; (process "),
            "{sent}"
        );
        std::fs::remove_dir_all(bin).unwrap();
    }

    #[tokio::test]
    async fn a_notification_that_cant_be_sent_is_recorded_and_ends_by_the_deadline() {
        let failing = notify_send(
            "notice-no-server",
            "echo 'Failed to connect to the notification server' >&2; exit 1",
        );
        let hanging = notify_send("notice-hanging", "exec /bin/sleep 10");
        let missing = crate::test_support::fresh_dir("notice-missing");
        let notices = Notices::default();
        for (bin, said) in [
            (&failing, "Failed to connect"),
            (&hanging, "within"),
            (&missing, "notify-send"),
        ] {
            let started = std::time::Instant::now();
            notices.send(place(bin), &label("pi"), None).await;
            assert!(started.elapsed() < DEADLINE + Duration::from_secs(2));
            let error = notices.last_error().unwrap_or_default();
            assert!(error.contains(said), "{said}: {error}");
        }
        // A later notification that goes through clears the error.
        let working = notify_send("notice-working", "true");
        notices.send(place(&working), &label("pi"), None).await;
        assert_eq!(notices.last_error(), None);
        for bin in [failing, hanging, missing, working] {
            std::fs::remove_dir_all(bin).unwrap();
        }
    }
}
