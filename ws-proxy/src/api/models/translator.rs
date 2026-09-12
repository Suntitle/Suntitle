use std::ffi::OsString;

use super::language::validate_language;
use super::{LanguageTag, default_true, require};
use clap_reverse::AsCommand as _;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema, Validate, kube::KubeSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TranslatorSettings {
    /// Language code of the input text.
    #[validate(custom(function = "validate_language"))]
    #[x_kube(validation = kube::core::Rule::new("self.matches('^[a-z]{2,3}$')").message("sourceLanguage must be a lowercase ISO 639 code"))]
    pub source_language: LanguageTag,
    /// Language code requested for the translated text.
    #[validate(custom(function = "validate_language"))]
    #[x_kube(validation = kube::core::Rule::new("self.matches('^[a-z]{2,3}$')").message("targetLanguage must be a lowercase ISO 639 code"))]
    pub target_language: LanguageTag,
    /// Translation backend used to process text.
    #[schema(min_length = 1, examples("openai"))]
    #[validate(custom(function = "validate_provider"))]
    #[schemars(extend("enum" = ["huggingface"]))]
    #[x_kube(validation = kube::core::Rule::new("self == 'huggingface'").message("only huggingface is supported"))]
    pub provider: String,
    /// Translation model override; null selects the provider default.
    #[schema(nullable)]
    #[schemars(length(max = 256))]
    pub model: Option<String>,
    /// Whether to retain input text formatting in translations.
    #[serde(default = "default_true")]
    #[schema(default = true)]
    pub preserve_formatting: bool,
}

fn validate_provider(value: &str) -> Result<(), ValidationError> {
    require(value == "huggingface", "only huggingface is supported")
}

#[derive(clap_reverse::ClapReverse)]
#[clap_reverse(binary = "suntitle-translator.py", delimiter = "=")]
struct TranslatorArgs {
    host: String,
    #[clap_reverse(name = "source-language")]
    source_language: String,
    #[clap_reverse(name = "target-language")]
    target_language: String,
    provider: String,
    model: String,
    #[clap_reverse(flag, name = "no-preserve-formatting")]
    no_preserve_formatting: bool,
}

impl TranslatorSettings {
    /// Builds script arguments for settings already validated at API submission.
    pub fn command_args(&self, host: &str) -> Vec<OsString> {
        let args = TranslatorArgs {
            host: host.to_owned(),
            source_language: self.source_language.0.clone(),
            target_language: self.target_language.0.clone(),
            provider: self.provider.clone(),
            model: self.model.clone().unwrap_or_default(),
            no_preserve_formatting: !self.preserve_formatting,
        };

        let cmd = args.as_command();
        std::iter::once(cmd.get_program().to_os_string())
            .chain(cmd.get_args().map(OsString::from))
            .collect()
    }
}
