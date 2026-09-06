use std::ffi::OsString;

use super::NodeBase;
use super::require;
use clap_reverse::AsCommand as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DisplayNode {
    #[serde(flatten)]
    pub base: NodeBase,
    pub settings: DisplaySettings,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DisplayTheme {
    Light,
    #[default]
    Dark,
    Transparent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplaySettings {
    #[schema(minimum = 1, maximum = 10)]
    #[validate(range(min = 1, max = 10))]
    pub max_lines: u8,
    #[schema(minimum = 0.5, maximum = 4.0)]
    #[validate(range(min = 0.5, max = 4.0), custom(function = "validate_finite"))]
    pub font_scale: f64,
    pub show_original: bool,
    #[serde(default)]
    #[schema(default = "dark")]
    #[schema(inline)]
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
            .collect()
    }
}
