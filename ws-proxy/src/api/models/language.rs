use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa::openapi::RefOr;
use utoipa::openapi::schema::{ObjectBuilder, Schema, Type};
use validator::ValidationError;

use super::require;

/// BCP 47 language tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct LanguageTag(
    /// Language code used to select speech or text processing languages.
    #[schemars(regex(pattern = "^[a-z]{2,3}$"), length(max = 3))]
    pub String,
);

impl utoipa::PartialSchema for LanguageTag {
    fn schema() -> RefOr<Schema> {
        ObjectBuilder::new()
            .schema_type(Type::String)
            .pattern(Some("^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})*$"))
            .description(Some("BCP 47 language tag."))
            .examples(["en", "en-US"])
            .into()
    }
}

impl ToSchema for LanguageTag {}

pub(super) fn validate_language(value: &LanguageTag) -> Result<(), ValidationError> {
    require(
        (2..=3).contains(&value.0.len()) && value.0.bytes().all(|byte| byte.is_ascii_lowercase()),
        "must be a lowercase ISO 639 language code; regional BCP 47 tags are not supported",
    )
}
