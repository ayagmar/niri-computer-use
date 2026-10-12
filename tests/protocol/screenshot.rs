//! Screenshots: the image block, the metadata beside it, and grim's arguments for each
//! target, format and scale.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::client::{Server, mistake, tool_error};
use crate::fixture::{Fixture, jpeg, png};
use crate::niri::{Niri, output};

/// The metadata, after checking the image block carries exactly `image` as `mime` and the
/// text block repeats the structured content.
fn delivered(result: &Value, image: &[u8], mime: &str) -> Value {
    assert_eq!(result["isError"], false, "{result}");
    let content = result["content"].as_array().unwrap();
    assert_eq!(content.len(), 2, "{result}");
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["mimeType"], mime);
    let data = base64::engine::general_purpose::STANDARD
        .decode(content[0]["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(data, image);
    let text: Value = serde_json::from_str(content[1]["text"].as_str().unwrap()).unwrap();
    let metadata = result["structuredContent"].clone();
    assert_eq!(text, metadata);
    assert_eq!(metadata["mime_type"], mime);
    metadata
}

#[tokio::test]
async fn the_focused_output_is_a_jpeg_lowered_to_1280_pixels_by_default() {
    let fixture = Fixture::new("shot-default");
    let _niri = Niri::start(&fixture);
    let image = jpeg(1280, 720, b"pixels");
    fixture.grim(&image);
    let mut server = Server::start(&fixture).await;
    let result = server
        .call("screenshot", json!({"target": "focused_output"}))
        .await;
    let metadata = delivered(&result, &image, "image/jpeg");
    assert_eq!(metadata["output"], "DP-1");
    assert_eq!(metadata["transform"], "Normal");
    assert_eq!(metadata["output_origin"], json!([0, 0]));
    assert_eq!(
        metadata["captured"],
        json!({"x": 0, "y": 0, "width": 2560, "height": 1440})
    );
    assert_eq!(metadata["scale"], 0.5);
    assert_eq!(
        (&metadata["width"], &metadata["height"]),
        (&json!(1280), &json!(720))
    );
    assert!(metadata["captured_at_unix_ms"].as_u64().unwrap() > 0);
    assert_eq!(
        fixture.args("grim"),
        ["-t", "jpeg", "-q", "80", "-s", "0.5", "-o", "DP-1", "-"]
    );
}

#[tokio::test]
async fn a_wide_enough_max_width_keeps_the_native_scale() {
    let fixture = Fixture::new("shot-native");
    let _niri = Niri::start(&fixture);
    let image = png(2560, 1440);
    fixture.grim(&image);
    let mut server = Server::start(&fixture).await;
    let result = server
        .call(
            "screenshot",
            json!({"target": "output:DP-1", "format": "png", "max_width": 4000}),
        )
        .await;
    let metadata = delivered(&result, &image, "image/png");
    assert_eq!(metadata["scale"], 1.0);
    assert_eq!(
        fixture.args("grim"),
        ["-t", "png", "-s", "1", "-o", "DP-1", "-"]
    );
}

#[tokio::test]
async fn a_region_on_a_fractional_output_left_of_the_origin_truncates_like_grim() {
    let fixture = Fixture::new("shot-region");
    let niri = Niri::start(&fixture);
    niri.set_outputs(
        vec![
            output("DP-1", Some((0, 0, 2560, 1440, 1.0))),
            output("HDMI-A-1", Some((-1280, 0, 1280, 720, 1.5))),
        ],
        Some("DP-1"),
    );
    // 101 x 51 logical pixels at 1.5 is 151.5 x 76.5, which grim truncates.
    let image = png(151, 76);
    fixture.grim(&image);
    let mut server = Server::start(&fixture).await;
    let region = json!({"x": -1279, "y": 10, "width": 101, "height": 51});
    let result = server
        .call(
            "screenshot",
            json!({"target": "region", "region": region, "format": "png"}),
        )
        .await;
    let metadata = delivered(&result, &image, "image/png");
    assert_eq!(metadata["output"], "HDMI-A-1");
    assert_eq!(metadata["output_origin"], json!([-1280, 0]));
    assert_eq!(metadata["captured"], region);
    assert_eq!(metadata["scale"], 1.5);
    assert_eq!(
        fixture.args("grim"),
        ["-t", "png", "-s", "1.5", "-g", "-1279,10 101x51", "-"]
    );
}

#[tokio::test]
async fn a_lowered_fractional_scale_gives_exactly_max_width() {
    let fixture = Fixture::new("shot-lowered");
    let niri = Niri::start(&fixture);
    niri.set_outputs(
        vec![output("eDP-1", Some((0, 0, 1707, 1067, 1.5)))],
        Some("eDP-1"),
    );
    let mut server = Server::start(&fixture).await;
    // 1707 x 1.5 is 2560 image pixels; 1000 of them need a scale near 0.5858.
    let height = 1067.0_f64 * 1000.0 / 1707.0;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a small positive height, truncated as grim does"
    )]
    let image = jpeg(1000, height as u16, &[]);
    fixture.grim(&image);
    let result = server
        .call(
            "screenshot",
            json!({"target": "focused_output", "max_width": 1000}),
        )
        .await;
    let metadata = delivered(&result, &image, "image/jpeg");
    assert_eq!(metadata["width"], 1000);
    let scale = metadata["scale"].as_f64().unwrap();
    assert!((scale - 1000.0 / 1707.0).abs() < 1e-9, "{scale}");
}

/// A grim that logs each call's arguments on a line and writes a 2560x1440 PNG at the
/// output's own scale, or a 1280-wide JPEG otherwise.
fn two_sizes(fixture: &Fixture) -> (Vec<u8>, Vec<u8>) {
    let (full, small) = (png(2560, 1440), jpeg(1280, 720, b"small"));
    std::fs::write(fixture.path("full.png"), &full).unwrap();
    std::fs::write(fixture.path("small.jpg"), &small).unwrap();
    fixture.program(
        "grim",
        r#"echo "$*" >> "$DIR/grim.calls"
case "$*" in
  "-t png -s 1 "*) cat "$DIR/full.png" ;;
  *) cat "$DIR/small.jpg" ;;
esac"#,
    );
    (full, small)
}

fn grim_calls(fixture: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fixture.path("grim.calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn policy(fixture: &Fixture, text: &str) {
    std::fs::create_dir_all(fixture.path("config/niri-computer-use")).unwrap();
    std::fs::write(fixture.path("config/niri-computer-use/policy.toml"), text).unwrap();
}

#[tokio::test]
async fn saving_is_refused_without_a_capture_dir_and_captures_nothing() {
    let fixture = Fixture::new("shot-nosave");
    let _niri = Niri::start(&fixture);
    two_sizes(&fixture);
    let mut server = Server::start(&fixture).await;
    let result = server
        .call(
            "screenshot",
            json!({"target": "focused_output", "save_path": "a.png"}),
        )
        .await;
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["error"], "save_not_enabled");
    assert_eq!(grim_calls(&fixture), Vec::<String>::new());
}

#[tokio::test]
async fn a_save_is_a_full_resolution_png_inside_the_capture_dir() {
    let mut fixture = Fixture::new("shot-save");
    let _niri = Niri::start(&fixture);
    let (full, small) = two_sizes(&fixture);
    fixture.set("HOME", fixture.dir.clone());
    policy(&fixture, "capture_dir = \"~/shots\"\n");
    let mut server = Server::start(&fixture).await;
    let save = |path: &str| json!({"target": "focused_output", "save_path": path});
    let result = server.call("screenshot", save("one.png")).await;
    let metadata = delivered(&result, &small, "image/jpeg");
    let saved = fixture.path("shots/one.png");
    assert_eq!(
        metadata["saved"],
        json!({"path": saved, "width": 2560, "height": 1440})
    );
    assert_eq!(std::fs::read(&saved).unwrap(), full);
    assert_eq!(
        grim_calls(&fixture),
        ["-t png -s 1 -o DP-1 -", "-t jpeg -q 80 -s 0.5 -o DP-1 -"]
    );
    let again = server.call("screenshot", save("one.png")).await;
    let text = again["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("already exists"), "{again}");
    assert_eq!(std::fs::read(&saved).unwrap(), full);
    let calls = grim_calls(&fixture).len();
    for escape in ["../one.png", "/tmp/one.png", "one.jpg"] {
        let refused = server.call("screenshot", save(escape)).await;
        assert!(
            refused["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("invalid arguments: "),
            "{refused}"
        );
    }
    assert_eq!(
        grim_calls(&fixture).len(),
        calls,
        "a refused path captures nothing"
    );
}

#[tokio::test]
async fn a_session_runs_at_most_sixteen_tool_calls_at_once() {
    use std::time::Duration;

    let fixture = Fixture::new("shot-in-flight");
    let _niri = Niri::start(&fixture);
    std::fs::write(fixture.path("grim.out"), jpeg(1280, 720, b"pixels")).unwrap();
    fixture.program(
        "grim",
        r#"echo >> "$DIR/grim.started"; await_file go; cat "$DIR/grim.out""#,
    );
    let mut server = Server::start(&fixture).await;
    let target = json!({"target": "focused_output"});
    let mut running = Vec::new();
    for _ in 0..16 {
        running.push(server.start_call("screenshot", target.clone()).await);
    }
    let started = || {
        std::fs::read_to_string(fixture.path("grim.started"))
            .map_or(0, |started| started.lines().count())
    };
    assert!(crate::fixture::eventually(Duration::from_secs(5), || started() == 16).await);
    let refused = server.start_call("screenshot", target.clone()).await;
    let response = server.response(refused).await;
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("16 tool calls running"), "{response}");
    std::fs::write(fixture.path("go"), "").unwrap();
    for id in running {
        assert_eq!(server.response(id).await["result"]["isError"], false);
    }
    assert_eq!(server.call("screenshot", target).await["isError"], false);
}

/// Zooming is read-only, so it works in the `observe` profile, where no lease is ever
/// held: a screenshot gets a ref anyway, and a region of its pixels comes back at the
/// output's own scale, with a ref of its own, while the first ref still works.
#[tokio::test]
async fn a_zoom_captures_a_region_of_an_earlier_screenshot_at_full_resolution() {
    let mut fixture = Fixture::new("shot-zoom");
    fixture.set("NIRI_COMPUTER_USE_TOOLS", "observe");
    let mut niri = Niri::start(&fixture);
    let (small, zoomed) = (jpeg(1280, 720, b"small"), jpeg(128, 64, b"zoomed"));
    zooming_grim(&fixture, &small, &zoomed);
    let mut server = Server::start(&fixture).await;
    // A ref needs the event stream, so that a reconnect can expire it.
    let stream = niri.stream().await;
    stream.initial(&[]);
    server.structured("desktop_state").await;
    let whole = server
        .call("screenshot", json!({"target": "focused_output"}))
        .await;
    let whole = delivered(&whole, &small, "image/jpeg");
    assert_eq!(whole["scale"], 0.5);
    let first = whole["screenshot_ref"].as_str().unwrap().to_owned();

    let zoom = json!({
        "target": "region",
        "screenshot_ref": first,
        "region": {"x": 100, "y": 50, "width": 64, "height": 32},
    });
    let zoomed_in = server.call("screenshot", zoom.clone()).await;
    let metadata = delivered(&zoomed_in, &zoomed, "image/jpeg");
    assert_eq!(
        metadata["captured"],
        json!({"x": 200, "y": 100, "width": 128, "height": 64})
    );
    assert_eq!(metadata["scale"], 1.0);
    let own = metadata["screenshot_ref"].as_str().unwrap();
    assert_ne!(own, first);
    let again = server.call("screenshot", zoom).await;
    delivered(&again, &zoomed, "image/jpeg");
    assert_eq!(
        grim_calls(&fixture)[1],
        "-t jpeg -q 80 -s 1 -g 200,100 128x64 -"
    );

    let outside = json!({
        "target": "region",
        "screenshot_ref": first,
        "region": {"x": 1270, "y": 0, "width": 64, "height": 32},
    });
    let refused = server.call("screenshot", outside).await;
    assert_eq!(
        refused["structuredContent"]["error"], "ref_invalid",
        "{refused}"
    );
    // A ref with another target would be ignored, so it is a mistake instead.
    let whole_again = json!({"target": "focused_output", "screenshot_ref": first});
    let message = mistake(&server.call("screenshot", whole_again).await);
    assert!(message.contains("needs target `region`"), "{message}");
    let negative = json!({
        "target": "region",
        "screenshot_ref": first,
        "region": {"x": -1, "y": 0, "width": 64, "height": 32},
    });
    let negative = mistake(&server.call("screenshot", negative).await);
    assert!(negative.contains("can't be negative"), "{negative}");
    assert_eq!(grim_calls(&fixture).len(), 3);
}

/// A grim that records its arguments and writes `zoomed` for a region, `whole` otherwise.
fn zooming_grim(fixture: &Fixture, whole: &[u8], zoomed: &[u8]) {
    std::fs::write(fixture.path("whole.jpg"), whole).unwrap();
    std::fs::write(fixture.path("zoomed.jpg"), zoomed).unwrap();
    fixture.program(
        "grim",
        r#"echo "$*" >> "$DIR/grim.calls"
case "$*" in
  *" -g "*) cat "$DIR/zoomed.jpg" ;;
  *) cat "$DIR/whole.jpg" ;;
esac"#,
    );
}

/// The ref of a screenshot of `target`, once the server follows niri's event stream, which
/// a ref needs.
async fn screenshot_ref(server: &mut Server, target: &str) -> String {
    server.structured("desktop_state").await;
    let whole = server.call("screenshot", json!({"target": target})).await;
    assert_eq!(whole["isError"], false, "{whole}");
    whole["structuredContent"]["screenshot_ref"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A zoom is at the output's own scale unless `max_width` caps it, even when that is wider
/// than the 1280 pixels a screenshot gets by default, and it lands on the screenshot's
/// output wherever that output sits in the layout.
#[tokio::test]
async fn a_zoom_is_at_its_outputs_own_scale_wherever_the_output_is() {
    let fixture = Fixture::new("shot-zoom-native");
    let mut niri = Niri::start(&fixture);
    niri.set_outputs(
        vec![
            output("DP-1", Some((0, 0, 1920, 1080, 1.0))),
            output("DP-2", Some((1920, 0, 1600, 900, 2.0))),
        ],
        Some("DP-1"),
    );
    let zoomed = jpeg(3000, 1000, b"zoomed");
    zooming_grim(&fixture, &jpeg(1280, 720, b"whole"), &zoomed);
    let mut server = Server::start(&fixture).await;
    let stream = niri.stream().await;
    stream.initial(&[]);
    let first = screenshot_ref(&mut server, "output:DP-2").await;

    let zoom = json!({
        "target": "region",
        "screenshot_ref": first,
        "region": {"x": 0, "y": 0, "width": 1200, "height": 400},
    });
    let metadata = delivered(
        &server.call("screenshot", zoom).await,
        &zoomed,
        "image/jpeg",
    );
    assert_eq!(metadata["output"], "DP-2");
    assert_eq!(
        metadata["captured"],
        json!({"x": 1920, "y": 0, "width": 1500, "height": 500})
    );
    assert_eq!(
        (&metadata["scale"], &metadata["width"]),
        (&json!(2.0), &json!(3000))
    );
    assert_eq!(
        grim_calls(&fixture)[1],
        "-t jpeg -q 80 -s 2 -g 1920,0 1500x500 -"
    );
}

/// A zoom captures the output its screenshot showed. Outputs that move while it is taken,
/// so that its region lies on another output, refuse the zoom rather than return that
/// output's pixels: whether they swap places at the reading the capture is planned from,
/// or while grim runs, after that reading.
#[tokio::test]
async fn a_zoom_whose_output_moves_during_the_capture_is_refused() {
    let fixture = Fixture::new("shot-zoom-moved");
    let mut niri = Niri::start(&fixture);
    let side_by_side = |left: &str, right: &str| {
        vec![
            output(left, Some((0, 0, 1280, 720, 1.0))),
            output(right, Some((1280, 0, 1280, 720, 1.0))),
        ]
    };
    niri.set_outputs(side_by_side("DP-1", "DP-2"), Some("DP-1"));
    zooming_grim(
        &fixture,
        &jpeg(1280, 720, b"whole"),
        &jpeg(64, 32, b"zoomed"),
    );
    // grim waits at the hold point `grim` once it has started.
    fixture.program(
        "grim",
        r#"echo "$*" >> "$DIR/grim.calls"
pass grim
case "$*" in
  *" -g "*) cat "$DIR/zoomed.jpg" ;;
  *) cat "$DIR/whole.jpg" ;;
esac"#,
    );
    let mut server = Server::start(&fixture).await;
    fixture.serve_programs();
    let stream = niri.stream().await;
    stream.initial(&[]);
    let first = screenshot_ref(&mut server, "output:DP-1").await;
    let zoom = json!({
        "target": "region",
        "screenshot_ref": first,
        "region": {"x": 100, "y": 50, "width": 64, "height": 32},
    });
    let points = fixture.points();
    for (point, grim_ran) in [("niri:Outputs", false), ("grim", true)] {
        niri.set_outputs(side_by_side("DP-1", "DP-2"), Some("DP-1"));
        let calls = grim_calls(&fixture).len();
        points.start();
        let held = points.hold(point, 1);
        let id = server.start_call("screenshot", zoom.clone()).await;
        held.arrived().await;
        niri.set_outputs(side_by_side("DP-2", "DP-1"), Some("DP-1"));
        held.release();
        let response = server.response(id).await;
        let (name, detail) = tool_error(&response["result"]);
        assert_eq!(name, "ref_invalid", "{point}: {detail}");
        assert!(detail.starts_with("output_changed"), "{point}: {detail}");
        assert_eq!(grim_calls(&fixture).len(), calls + usize::from(grim_ran));
    }
}
