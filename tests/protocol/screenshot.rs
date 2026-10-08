//! Screenshots: the image block, the metadata beside it, and grim's arguments for each
//! target, format and scale.

use base64::Engine as _;
use serde_json::{Value, json};

use crate::client::Server;
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
