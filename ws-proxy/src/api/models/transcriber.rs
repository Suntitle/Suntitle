use std::ffi::OsString;

use super::language::validate_language;
use super::{LanguageTag, default_true, require};
use clap_reverse::AsCommand as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

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
pub enum TranscriberTask {
    #[default]
    Transcribe,
    Translate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate, kube::KubeSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranscriberSettings {
    /// Speech recognition model used to decode audio.
    #[schema(min_length = 1, examples("large-v3"))]
    #[validate(custom(function = "validate_model"))]
    #[schemars(length(max = 256))]
    #[x_kube(validation = kube::core::Rule::new(concat!(
        "self in ['tiny.en','tiny','base.en','base','small.en','small',",
        "'medium.en','medium','large-v1','large-v2','large-v3','large','large-v3-turbo']",
    )).message("unsupported transcriber model"))]
    pub model: String,
    /// Language code of the incoming speech.
    #[validate(custom(function = "validate_language"))]
    #[x_kube(validation = kube::core::Rule::new("self.matches('^[a-z]{2,3}$')").message("language must be a lowercase ISO 639 code"))]
    pub language: LanguageTag,
    /// Whether to transcribe speech or translate it into English.
    #[serde(default)]
    #[schema(default = "transcribe")]
    #[schema(inline)]
    #[x_kube(validation = kube::core::Rule::new("self in ['transcribe', 'translate']").message("unsupported transcriber task"))]
    pub task: TranscriberTask,
    /// Number of candidate sequences retained during decoding.
    #[serde(default = "default_beam_size")]
    #[schema(minimum = 1, maximum = 20, default = 5)]
    #[validate(range(min = 1, max = 20))]
    #[x_kube(validation = kube::core::Rule::new("self >= 1 && self <= 20").message("beamSize must be between 1 and 20"))]
    pub beam_size: u8,
    /// Whether voice activity detection filters out non-speech audio.
    #[serde(default = "default_true")]
    #[schema(default = true)]
    pub vad_enabled: bool,
}

const fn default_beam_size() -> u8 {
    5
}

fn validate_model(value: &str) -> Result<(), ValidationError> {
    require(
        [
            "tiny.en",
            "tiny",
            "base.en",
            "base",
            "small.en",
            "small",
            "medium.en",
            "medium",
            "large-v1",
            "large-v2",
            "large-v3",
            "large",
            "large-v3-turbo",
        ]
        .contains(&value),
        "unsupported transcriber model",
    )
}

#[derive(clap_reverse::ClapReverse)]
#[clap_reverse(binary = "whisper_online_client.py", delimiter = "=")]
struct TranscriberArgs {
    uri: String,
    model: String,
    language: String,
    task: &'static str,
    #[clap_reverse(flag)]
    vad: bool,
    #[clap_reverse(name = "beam-size")]
    beam_size: u8,
}

impl TranscriberSettings {
    /// Builds script arguments for settings already validated at API submission.
    pub fn command_args(&self, transcriber_url: &str) -> Vec<OsString> {
        let args = TranscriberArgs {
            uri: transcriber_url.to_owned(),
            model: self.model.clone(),
            language: self.language.0.clone(),
            task: match self.task {
                TranscriberTask::Transcribe => "transcribe",
                TranscriberTask::Translate => "translate",
            },
            vad: self.vad_enabled,
            beam_size: self.beam_size,
        };

        let cmd = args.as_command();
        std::iter::once(cmd.get_program().to_os_string())
            .chain(cmd.get_args().map(OsString::from))
            .collect()
    }
}
