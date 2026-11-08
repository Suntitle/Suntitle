use std::ffi::OsString;

use super::require;
use clap_reverse::AsCommand as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

/// Broadcast stream consumed by the display. Translation is the legacy default.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    ToSchema,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DisplayInputStream {
    /// Raw text broadcast to clients connected to `/translate`.
    Transcription,
    /// Translated text broadcast to clients connected to `/display`.
    #[default]
    Translation,
}

impl DisplayInputStream {
    /// Omit the legacy default from serialized settings for compatibility.
    fn is_translation(&self) -> bool {
        *self == Self::Translation
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    ToSchema,
    schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum DisplayTheme {
    Light,
    #[default]
    Dark,
    Transparent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate, kube::KubeSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplaySettings {
    /// Stream selected by the UI from the upstream node's role.
    /// This selects a broadcast stage; it MUST NOT imply isolation by node identity.
    #[serde(default, skip_serializing_if = "DisplayInputStream::is_translation")]
    #[schema(default = "translation")]
    #[x_kube(validation = kube::core::Rule::new("self in ['transcription', 'translation']").message("unsupported display input stream"))]
    pub input_stream: DisplayInputStream,
    /// Maximum number of subtitle lines displayed at once.
    #[schema(minimum = 1, maximum = 10)]
    #[validate(range(min = 1, max = 10))]
    #[x_kube(validation = kube::core::Rule::new("self >= 1 && self <= 10").message("maxLines must be between 1 and 10"))]
    pub max_lines: u8,
    /// Multiplier applied to the default subtitle font size.
    #[schema(minimum = 0.5, maximum = 4.0)]
    #[validate(range(min = 0.5, max = 4.0), custom(function = "validate_finite"))]
    #[x_kube(validation = kube::core::Rule::new("self >= 0.5 && self <= 4.0").message("fontScale must be between 0.5 and 4.0"))]
    pub font_scale: f64,
    /// Whether to show source text alongside its translation.
    pub show_original: bool,
    /// Color and background style used for subtitles.
    #[serde(default)]
    #[schema(default = "dark")]
    #[schema(inline)]
    #[x_kube(validation = kube::core::Rule::new("self in ['light', 'dark', 'transparent']").message("unsupported display theme"))]
    pub theme: DisplayTheme,
}

fn validate_finite(value: f64) -> Result<(), ValidationError> {
    require(value.is_finite(), "must be finite")
}

#[derive(clap_reverse::ClapReverse)]
#[clap_reverse(binary = "suntitle.py", delimiter = "=")]
struct DisplayArgs {
    host: String,
    #[clap_reverse(name = "max-lines")]
    max_lines: u8,
    #[clap_reverse(name = "font-scale")]
    font_scale: f64,
    #[clap_reverse(flag, name = "show-original")]
    show_original: bool,
    theme: &'static str,
}

impl DisplaySettings {
    /// Builds script arguments for settings already validated at API submission.
    pub fn command_args(&self, host: &str) -> Vec<OsString> {
        let args = DisplayArgs {
            host: host.to_owned(),
            max_lines: self.max_lines,
            font_scale: self.font_scale,
            show_original: self.show_original,
            theme: match self.theme {
                DisplayTheme::Light => "light",
                DisplayTheme::Dark => "dark",
                DisplayTheme::Transparent => "transparent",
            },
        };
        let cmd = args.as_command();
        std::iter::once(cmd.get_program().to_os_string())
            .chain(cmd.get_args().map(OsString::from))
            .chain(
                (self.input_stream == DisplayInputStream::Transcription)
                    .then(|| OsString::from("--input-stream=transcription")),
            )
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Legacy settings retain their command; transcription adds only the stream selector.
    #[test]
    fn stream_selection_preserves_legacy_defaults() {
        let mut settings: DisplaySettings = serde_json::from_value(serde_json::json!({
            "maxLines":3, "fontScale":1.0, "showOriginal":false,
        }))
        .unwrap();
        assert_eq!(settings.input_stream, DisplayInputStream::Translation);
        assert!(
            serde_json::to_value(&settings)
                .unwrap()
                .get("inputStream")
                .is_none()
        );
        let mut expected = settings.command_args("localhost:3030");
        expected.push("--input-stream=transcription".into());
        settings.input_stream = DisplayInputStream::Transcription;
        assert_eq!(settings.command_args("localhost:3030"), expected);
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["inputStream"],
            "transcription"
        );
    }
}
