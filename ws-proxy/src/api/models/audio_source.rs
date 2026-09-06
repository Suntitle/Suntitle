use std::ffi::OsString;

use super::NodeBase;
use super::require;
use clap_reverse::AsCommand as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use validator::{Validate, ValidationError};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AudioSourceNode {
    #[serde(flatten)]
    pub base: NodeBase,
    pub settings: AudioSourceSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioSourceSettings {
    #[schema(schema_with = sample_rate_schema)]
    #[validate(custom(function = "validate_sample_rate"))]
    pub sample_rate_hz: u32,
    #[schema(minimum = 1, maximum = 2)]
    #[validate(range(min = 1, max = 2))]
    pub channels: u8,
    #[schema(inline)]
    #[validate(custom(function = "validate_encoding"))]
    pub encoding: AudioEncoding,
    #[schema(nullable, max_length = 256)]
    #[validate(length(max = 256))]
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
