//! The prompt `confirm_lease` puts on the desktop before a lease is granted: a
//! notification with Allow and Deny, shown by `notify-send` through the session's
//! notification server, which the user answers on the desktop itself. Only Allow grants
//! the lease; Deny, a dismissed or expired notification, and no answer by the deadline all
//! decline it.

use std::time::Duration;

use crate::error::{ErrorName, ToolError};
use crate::runner::{self, Place, Search};
use crate::session::Label;

/// How long the user has to answer. The notification expires then, and `notify-send` is
/// stopped if the notification server keeps it.
pub(crate) const DEADLINE: Duration = Duration::from_secs(60);
/// The action key `notify-send` prints when the user chooses Allow.
const ALLOW: &str = "allow";
const DENY: &str = "deny";
/// Far above an action key.
const MAX_STDOUT: u64 = 1024;

/// Asks the user to allow the client `label` to take the lease; `lease_declined` unless
/// they choose Allow within `DEADLINE`. `notify-send` is found in `place`, the session's.
pub(crate) async fn ask(place: Place<'_>, label: &Label) -> Result<(), ToolError> {
    ask_within(place, label, DEADLINE).await
}

async fn ask_within(place: Place<'_>, label: &Label, deadline: Duration) -> Result<(), ToolError> {
    let ran = runner::run(
        "notify-send",
        Search::Client(place),
        &args(label, deadline),
        deadline,
        MAX_STDOUT,
    )
    .await;
    let done = match ran {
        Ok(done) => done,
        Err(error) if error.name == ErrorName::DeadlineExceeded => {
            return Err(declined(&format!(
                "the user didn't answer within {} s",
                deadline.as_secs()
            )));
        }
        Err(error) => return Err(error),
    };
    if !done.status.success() {
        return Err(done.failure("notify-send"));
    }
    let answer = done.stdout.trim_ascii();
    if answer == ALLOW.as_bytes() {
        return Ok(());
    }
    if answer == DENY.as_bytes() {
        return Err(declined("the user chose Deny"));
    }
    Err(declined(&format!(
        "the prompt closed without an answer: dismissed, or not answered within {} s",
        deadline.as_secs()
    )))
}

fn declined(why: &str) -> ToolError {
    ToolError::new(
        ErrorName::LeaseDeclined,
        format!(
            "{why}; the lease wasn't granted. Ask the user before calling acquire_desktop again"
        ),
    )
}

/// `notify-send`'s arguments. The client names itself and nothing checks the name, so the
/// prompt says that, next to the process it started. The label holds no markup or line
/// break (`Label`).
fn args(label: &Label, deadline: Duration) -> Vec<String> {
    let body = format!(
        "A client that calls itself \"{}\", process {}, asks for the niri-computer-use lease. With it, the agent can click, type and change windows until it releases the lease or you press the stop key. No answer within {} s declines.",
        label.client(),
        label.pid(),
        deadline.as_secs()
    );
    vec![
        "--app-name=niri-computer-use".to_owned(),
        "--urgency=critical".to_owned(),
        format!("--expire-time={}", deadline.as_millis()),
        format!("--action={DENY}=Deny"),
        format!("--action={ALLOW}=Allow"),
        "Allow an agent to use this desktop?".to_owned(),
        body,
    ]
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::PathBuf;

    use super::*;

    /// A directory holding a `notify-send` that runs `body`.
    fn notify_send(name: &str, body: &str) -> PathBuf {
        let bin = crate::test_support::fresh_dir(name);
        let program = bin.join("notify-send");
        std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    async fn asked(bin: &std::path::Path, deadline: Duration) -> Result<(), ToolError> {
        let place = Place {
            path: Some(bin.as_os_str()),
            cwd: None,
        };
        let label = crate::test_support::session(1).label("claude-code");
        ask_within(place, &label, deadline).await
    }

    #[tokio::test]
    async fn only_allow_grants_the_lease() {
        let allowed = notify_send("confirm-allow", "echo allow");
        assert_eq!(asked(&allowed, DEADLINE).await, Ok(()));
        for (name, body) in [
            ("confirm-deny", "echo deny"),
            ("confirm-dismissed", "true"),
            ("confirm-index", "echo 1"),
        ] {
            let bin = notify_send(name, body);
            let error = asked(&bin, DEADLINE).await.unwrap_err();
            assert_eq!(error.name, ErrorName::LeaseDeclined, "{body}: {error:?}");
        }
    }

    #[tokio::test]
    async fn no_answer_by_the_deadline_declines() {
        let bin = notify_send("confirm-silent", "exec /bin/sleep 10");
        let started = std::time::Instant::now();
        let error = asked(&bin, Duration::from_millis(300)).await.unwrap_err();
        assert_eq!(error.name, ErrorName::LeaseDeclined, "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_prompt_that_cant_be_shown_keeps_notify_sends_error() {
        let bin = notify_send(
            "confirm-no-server",
            "echo 'Failed to connect to the notification server' >&2; exit 1",
        );
        let error = asked(&bin, DEADLINE).await.unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.contains("Failed to connect"), "{error:?}");
    }

    // Nothing checks the name a client gives itself, so the prompt doesn't vouch for it,
    // and markup or a line break in it shows as `_` rather than shaping the prompt.
    #[test]
    fn the_prompt_says_the_name_is_the_clients_own_and_cant_be_shaped_by_it() {
        let session = crate::test_support::session(1);
        let args = args(&session.label("<b>Firefox</b>\nallowed by you"), DEADLINE);
        let body = args.last().unwrap();
        let pid = std::process::id();
        assert!(
            body.starts_with(&format!(
                "A client that calls itself \"_b_Firefox__b__allowed by you\", process {pid}, asks"
            )),
            "{body}"
        );
    }
}
