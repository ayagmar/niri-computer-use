//! The two shapes of a failed call: an argument mistake is `isError` with plain text, and
//! an execution failure is `isError` with a stable `error` name and the upstream `detail`.

use serde_json::{Value, json};

use crate::client::{Server, mistake, tool_error};
use crate::fixture::{Fixture, jpeg};
use crate::niri::{Niri, output};

#[tokio::test]
async fn arguments_that_dont_fit_the_schema_are_text_and_never_reach_the_tool() {
    let fixture = Fixture::new("schema");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    for arguments in [
        json!({}),
        json!({"target": 7}),
        json!({"target": "focused_output", "format": "gif"}),
        json!({"target": "focused_output", "max_width": -1}),
        json!({"target": "region", "region": {"x": 0, "y": 0}}),
    ] {
        let result = server.call("screenshot", arguments.clone()).await;
        let text = mistake(&result);
        assert!(!text.is_empty(), "{arguments}");
    }
    assert_eq!(fixture.audit_lines(), Vec::<Value>::new());
}

#[tokio::test]
async fn arguments_that_dont_fit_the_desktop_are_text_and_logged_as_such() {
    let fixture = Fixture::new("desktop-args");
    let niri = Niri::start(&fixture);
    niri.set_outputs(
        vec![
            output("DP-1", Some((0, 0, 2560, 1440, 1.0))),
            output("HDMI-A-1", Some((-1280, 0, 1280, 720, 1.5))),
            output("DP-2", None),
        ],
        None,
    );
    let mut server = Server::start(&fixture).await;
    let cases = [
        (
            json!({"target": "output:NOPE"}),
            "no enabled output named \"NOPE\"",
        ),
        (
            json!({"target": "output:DP-2"}),
            "no enabled output named \"DP-2\"",
        ),
        (
            json!({"target": "focused_output"}),
            "niri reports no focused output",
        ),
        (json!({"target": "nowhere"}), "unknown target"),
        (json!({"target": "region"}), "needs a `region` rectangle"),
        (
            json!({"target": "region", "region": {"x": -10, "y": 0, "width": 20, "height": 10}}),
            "is not inside one output",
        ),
        (
            json!({"target": "region", "region": {"x": 0, "y": 0, "width": 0, "height": 10}}),
            "is not inside one output",
        ),
        (
            json!({"target": "output:DP-1", "max_width": 0}),
            "`max_width` must be at least 1",
        ),
    ];
    for (arguments, expected) in &cases {
        let text = mistake(&server.call("screenshot", arguments.clone()).await);
        assert!(text.starts_with("invalid arguments: "), "{text}");
        assert!(text.contains(expected), "{arguments}: {text}");
    }
    let lines = fixture.audit_lines();
    assert_eq!(lines.len(), cases.len());
    for line in &lines {
        assert_eq!(line["error"], "invalid_arguments", "{line}");
    }
}

#[tokio::test]
async fn without_niri_calls_fail_as_niri_unavailable() {
    let fixture = Fixture::new("no-niri");
    let mut server = Server::start(&fixture).await;
    for tool in ["outputs", "desktop_state"] {
        let (name, detail) = tool_error(&server.call(tool, json!({})).await);
        assert_eq!(name, "niri_unavailable", "{tool}: {detail}");
        assert_ne!(detail, "");
    }
    let status = server.structured("status").await;
    assert_eq!(status["niri"]["error"]["error"], "niri_unavailable");
    assert_eq!(status["niri"]["event_stream"], "disconnected");
    let errors: Vec<Value> = fixture
        .audit_lines()
        .into_iter()
        .map(|line| line["error"].clone())
        .collect();
    assert_eq!(
        errors,
        [
            json!("niri_unavailable"),
            json!("niri_unavailable"),
            Value::Null
        ]
    );
}

#[tokio::test]
async fn a_failing_grim_keeps_its_exit_status_and_the_start_of_its_stderr() {
    let fixture = Fixture::new("grim-fails");
    let _niri = Niri::start(&fixture);
    // 20 KiB of stderr, more than the 16 KiB kept, then a non-zero exit.
    fixture.program(
        "grim",
        "echo 'grim: first line' >&2; head -c 20480 /dev/zero | tr '\\0' x >&2; exit 3",
    );
    let mut server = Server::start(&fixture).await;
    let (name, detail) = tool_error(
        &server
            .call("screenshot", json!({"target": "focused_output"}))
            .await,
    );
    assert_eq!(name, "upstream_error");
    let start: String = detail.chars().take(80).collect();
    assert!(
        detail.starts_with("grim exited with exit status: 3: grim: first line"),
        "{start}"
    );
    assert!(detail.len() < 16 * 1024 + 100, "{}", detail.len());
    assert_eq!(fixture.audit_lines()[0]["error"], "upstream_error");
}

#[tokio::test]
async fn an_image_of_the_wrong_size_is_an_upstream_error() {
    let fixture = Fixture::new("bad-header");
    let _niri = Niri::start(&fixture);
    let mut server = Server::start(&fixture).await;
    for (image, expected) in [
        (
            jpeg(1279, 720, &[]),
            "Some((1279, 720)), expected (1280, 720)",
        ),
        (b"not an image".to_vec(), "None, expected (1280, 720)"),
    ] {
        fixture.grim(&image);
        let (name, detail) = tool_error(
            &server
                .call("screenshot", json!({"target": "focused_output"}))
                .await,
        );
        assert_eq!(name, "upstream_error");
        assert!(detail.contains(expected), "{detail}");
    }
}

#[tokio::test]
async fn clipboard_text_over_one_mebibyte_is_an_upstream_error() {
    let fixture = Fixture::new("big-clipboard");
    fixture.program("wl-paste", "head -c 1048577 /dev/zero | tr '\\0' x");
    let mut server = Server::start(&fixture).await;
    let (name, detail) = tool_error(&server.call("clipboard_read", json!({})).await);
    assert_eq!(name, "upstream_error");
    assert!(
        detail.starts_with("wl-paste wrote more than 1048576 bytes and exited with"),
        "{detail}"
    );
}

#[tokio::test]
async fn an_image_over_64_mebibytes_is_an_upstream_error() {
    let fixture = Fixture::new("big-image");
    let _niri = Niri::start(&fixture);
    fixture.program("grim", "head -c 67108865 /dev/zero");
    let mut server = Server::start(&fixture).await;
    let (name, detail) = tool_error(
        &server
            .call("screenshot", json!({"target": "focused_output"}))
            .await,
    );
    assert_eq!(name, "upstream_error");
    assert!(
        detail.starts_with("grim wrote more than 67108864 bytes and exited with"),
        "{detail}"
    );
}
