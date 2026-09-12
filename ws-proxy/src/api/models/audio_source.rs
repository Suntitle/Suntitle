use std::ffi::OsString;

use super::require;
use clap_reverse::AsCommand as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use validator::{Validate, ValidationError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate, kube::KubeSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioSourceSettings {
    /// Number of audio samples captured per second.
    #[schema(schema_with = sample_rate_schema)]
    #[validate(custom(function = "validate_sample_rate"))]
    #[schemars(extend("enum" = [8000, 16000, 22050, 44100, 48000]))]
    #[x_kube(validation = kube::core::Rule::new("self in [8000, 16000, 22050, 44100, 48000]").message("unsupported sample rate"))]
    pub sample_rate_hz: u32,
    /// Number of audio channels captured.
    #[schema(minimum = 1, maximum = 2)]
    #[validate(range(min = 1, max = 2))]
    #[x_kube(validation = kube::core::Rule::new("self >= 1 && self <= 2").message("channels must be between 1 and 2"))]
    pub channels: u8,
    /// Format used to encode captured audio.
    #[schema(inline)]
    #[validate(custom(function = "validate_encoding"))]
    #[schemars(extend("enum" = ["pcm_s16le"]))]
    #[x_kube(validation = kube::core::Rule::new("self == 'pcm_s16le'").message("only pcm_s16le is supported"))]
    pub encoding: AudioEncoding,
    /// Audio capture device; null selects the runtime default.
    #[schema(nullable, max_length = 256)]
    #[validate(length(max = 256))]
    pub device_name: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncoding {
    PcmS16le,
    PcmF32le,
    Opus,
}

fn sample_rate_schema() -> RefOr<Schema> {
    ObjectBuilder::new()
        .schema_type(Type::Integer)
        .enum_values(Some([8000, 16000, 22050, 44100, 48000]))
        .into()
}

fn validate_sample_rate(value: u32) -> Result<(), ValidationError> {
    require(
        [8000, 16000, 22050, 44100, 48000].contains(&value),
        "must be one of 8000, 16000, 22050, 44100, or 48000",
    )
}

fn validate_encoding(value: &AudioEncoding) -> Result<(), ValidationError> {
    require(
        *value == AudioEncoding::PcmS16le,
        "only pcm_s16le is supported",
    )
}

#[derive(clap_reverse::ClapReverse)]
#[clap_reverse(binary = "suntitle-listener.py", delimiter = "=")]
struct AudioSourceArgs {
    #[clap_reverse(transparent)]
    audio_url: String,
    samplerate: u32,
    channels: u8,
    device: String,
    format: &'static str,
}

impl AudioSourceSettings {
    /// Builds script arguments for settings already validated at API submission.
    pub fn command_args(&self, audio_url: &str) -> Vec<OsString> {
        let args = AudioSourceArgs {
            audio_url: audio_url.to_owned(),
            samplerate: self.sample_rate_hz,
            channels: self.channels,
            device: self.device_name.clone().unwrap_or_default(),
            format: "raw",
        };

        let cmd = args.as_command();
        std::iter::once(cmd.get_program().to_os_string())
            .chain(cmd.get_args().map(OsString::from))
            .collect()
    }
}
