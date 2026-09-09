use actix_web::test;
use serde_json::json;

#[test]
async fn numeric_boundaries_are_accepted_and_nonfinite_scales_rejected() {
    use validator::Validate;
    use ws_proxy::api::models::{AudioSourceSettings, DisplaySettings, TranscriberSettings};
    for rate in [8000, 16000, 22050, 44100, 48000] {
        for channels in [1, 2] {
            let settings: AudioSourceSettings = serde_json::from_value(
                json!({"sampleRateHz":rate,"channels":channels,"encoding":"pcm_s16le"}),
            )
            .unwrap();
            assert!(settings.validate().is_ok());
        }
    }
    for beam in [1, 20] {
        let settings: TranscriberSettings =
            serde_json::from_value(json!({"model":"tiny","language":"eng","beamSize":beam}))
                .unwrap();
        assert!(settings.validate().is_ok());
    }
    for lines in [1, 10] {
        for scale in [0.5, 4.0] {
            let mut settings: DisplaySettings = serde_json::from_value(
                json!({"maxLines":lines,"fontScale":scale,"showOriginal":false}),
            )
            .unwrap();
            assert!(settings.validate().is_ok());
            for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                settings.font_scale = invalid;
                assert!(settings.validate().is_err());
            }
        }
    }
}
