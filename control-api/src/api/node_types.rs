use actix_web::{get, web};

use super::errors::{ApiResult, Unauthorized};
use super::models::{
    NodeSettingDefinition, NodeSettingInput, NodeSettingOption, NodeSettingValue,
    NodeSettingValueType, NodeRole, NodeTypeDefinition, NodeTypeDefinitionList,
};

fn setting(
    key: &str,
    label: &str,
    input: NodeSettingInput,
    value_type: NodeSettingValueType,
    required: bool,
    default_value: NodeSettingValue,
) -> NodeSettingDefinition {
    NodeSettingDefinition {
        key: key.to_owned(),
        label: label.to_owned(),
        description: None,
        input,
        value_type,
        required,
        default_value: Some(default_value),
        placeholder: None,
        minimum: None,
        maximum: None,
        options: None,
    }
}

fn option(value: NodeSettingValue, label: &str) -> NodeSettingOption {
    NodeSettingOption {
        value,
        label: label.to_owned(),
    }
}

fn transcriber_definition() -> NodeTypeDefinition {
    let mut model = setting(
        "model",
        "Model",
        NodeSettingInput::Text,
        NodeSettingValueType::String,
        true,
        NodeSettingValue::String("large-v3".to_owned()),
    );
    model.description = Some("Whisper model loaded by the transcriber.".to_owned());
    model.placeholder = Some("large-v3".to_owned());

    let mut language = setting(
        "language",
        "Language",
        NodeSettingInput::Text,
        NodeSettingValueType::String,
        true,
        NodeSettingValue::String("en".to_owned()),
    );
    language.description = Some("BCP 47 language tag used for recognition.".to_owned());
    language.placeholder = Some("en".to_owned());

    let mut task = setting(
        "task",
        "Task",
        NodeSettingInput::Select,
        NodeSettingValueType::String,
        false,
        NodeSettingValue::String("transcribe".to_owned()),
    );
    task.options = Some(vec![
        option(
            NodeSettingValue::String("transcribe".to_owned()),
            "Transcribe",
        ),
        option(
            NodeSettingValue::String("translate".to_owned()),
            "Translate to English",
        ),
    ]);

    let mut beam_size = setting(
        "beamSize",
        "Beam size",
        NodeSettingInput::Number,
        NodeSettingValueType::Integer,
        false,
        NodeSettingValue::Number(5.0),
    );
    beam_size.minimum = Some(1.0);
    beam_size.maximum = Some(20.0);

    let mut vad_enabled = setting(
        "vadEnabled",
        "Enable voice activity detection",
        NodeSettingInput::Checkbox,
        NodeSettingValueType::Boolean,
        false,
        NodeSettingValue::Boolean(true),
    );
    vad_enabled.description = Some("Skip silent audio before transcription.".to_owned());

    NodeTypeDefinition {
        node_type: NodeRole::Transcriber,
        name: "Transcriber".to_owned(),
        description: "Turns incoming audio into timestamped text.".to_owned(),
        input_types: vec![NodeRole::AudioSource],
        settings: vec![model, language, task, beam_size, vad_enabled],
    }
}

fn audio_source_definition() -> NodeTypeDefinition {
    let mut sample_rate = setting(
        "sampleRateHz",
        "Sample rate",
        NodeSettingInput::Select,
        NodeSettingValueType::Integer,
        true,
        NodeSettingValue::Number(48_000.0),
    );
    sample_rate.options = Some(vec![
        option(NodeSettingValue::Number(8_000.0), "8 kHz"),
        option(NodeSettingValue::Number(16_000.0), "16 kHz"),
        option(NodeSettingValue::Number(22_050.0), "22.05 kHz"),
        option(NodeSettingValue::Number(44_100.0), "44.1 kHz"),
        option(NodeSettingValue::Number(48_000.0), "48 kHz"),
    ]);

    let mut channels = setting(
        "channels",
        "Channels",
        NodeSettingInput::Select,
        NodeSettingValueType::Integer,
        true,
        NodeSettingValue::Number(1.0),
    );
    channels.options = Some(vec![
        option(NodeSettingValue::Number(1.0), "Mono"),
        option(NodeSettingValue::Number(2.0), "Stereo"),
    ]);

    let mut encoding = setting(
        "encoding",
        "Encoding",
        NodeSettingInput::Select,
        NodeSettingValueType::String,
        true,
        NodeSettingValue::String("pcm_s16le".to_owned()),
    );
    encoding.options = Some(vec![option(
        NodeSettingValue::String("pcm_s16le".to_owned()),
        "PCM 16-bit",
    )]);

    let mut device_name = setting(
        "deviceName",
        "Device name",
        NodeSettingInput::Text,
        NodeSettingValueType::String,
        false,
        NodeSettingValue::String(String::new()),
    );
    device_name.placeholder = Some("USB Audio Device".to_owned());

    NodeTypeDefinition {
        node_type: NodeRole::AudioSource,
        name: "Audio source".to_owned(),
        description: "Captures audio from a device or stream.".to_owned(),
        input_types: vec![],
        settings: vec![sample_rate, channels, encoding, device_name],
    }
}

fn translator_definition() -> NodeTypeDefinition {
    let mut source_language = setting(
        "sourceLanguage",
        "Source language",
        NodeSettingInput::Text,
        NodeSettingValueType::String,
        true,
        NodeSettingValue::String("en".to_owned()),
    );
    source_language.placeholder = Some("en".to_owned());

    let mut target_language = setting(
        "targetLanguage",
        "Target language",
        NodeSettingInput::Text,
        NodeSettingValueType::String,
        true,
        NodeSettingValue::String("es".to_owned()),
    );
    target_language.placeholder = Some("es".to_owned());

    let mut provider = setting(
        "provider",
        "Provider",
        NodeSettingInput::Select,
        NodeSettingValueType::String,
        true,
        NodeSettingValue::String("huggingface".to_owned()),
    );
    provider.options = Some(vec![option(
        NodeSettingValue::String("huggingface".to_owned()),
        "Hugging Face",
    )]);

    let mut model = setting(
        "model",
        "Model",
        NodeSettingInput::Text,
        NodeSettingValueType::String,
        false,
        NodeSettingValue::String(String::new()),
    );
    model.placeholder = Some("Optional model override".to_owned());

    let preserve_formatting = setting(
        "preserveFormatting",
        "Preserve formatting",
        NodeSettingInput::Checkbox,
        NodeSettingValueType::Boolean,
        false,
        NodeSettingValue::Boolean(true),
    );

    NodeTypeDefinition {
        node_type: NodeRole::Translator,
        name: "Translator".to_owned(),
        description: "Translates transcription text into another language.".to_owned(),
        input_types: vec![NodeRole::Transcriber],
        settings: vec![
            source_language,
            target_language,
            provider,
            model,
            preserve_formatting,
        ],
    }
}

fn display_definition() -> NodeTypeDefinition {
    let mut max_lines = setting(
        "maxLines",
        "Maximum lines",
        NodeSettingInput::Number,
        NodeSettingValueType::Integer,
        true,
        NodeSettingValue::Number(3.0),
    );
    max_lines.minimum = Some(1.0);
    max_lines.maximum = Some(10.0);

    let mut font_scale = setting(
        "fontScale",
        "Font scale",
        NodeSettingInput::Number,
        NodeSettingValueType::Number,
        true,
        NodeSettingValue::Number(1.0),
    );
    font_scale.minimum = Some(0.5);
    font_scale.maximum = Some(4.0);

    let show_original = setting(
        "showOriginal",
        "Show original text",
        NodeSettingInput::Checkbox,
        NodeSettingValueType::Boolean,
        true,
        NodeSettingValue::Boolean(true),
    );

    let mut theme = setting(
        "theme",
        "Theme",
        NodeSettingInput::Select,
        NodeSettingValueType::String,
        false,
        NodeSettingValue::String("dark".to_owned()),
    );
    theme.options = Some(vec![
        option(NodeSettingValue::String("light".to_owned()), "Light"),
        option(NodeSettingValue::String("dark".to_owned()), "Dark"),
        option(
            NodeSettingValue::String("transparent".to_owned()),
            "Transparent",
        ),
    ]);

    NodeTypeDefinition {
        node_type: NodeRole::Display,
        name: "Display".to_owned(),
        description: "Presents transcription and translation output.".to_owned(),
        input_types: vec![NodeRole::Transcriber, NodeRole::Translator],
        settings: vec![max_lines, font_scale, show_original, theme],
    }
}

fn node_type_catalog() -> NodeTypeDefinitionList {
    let items = vec![
        transcriber_definition(),
        audio_source_definition(),
        translator_definition(),
        display_definition(),
    ];
    let total = items.len() as u64;

    NodeTypeDefinitionList { items, total }
}

#[utoipa::path(
    get,
    path = "/node-types",
    tag = "Node types",
    operation_id = "listNodeTypes",
    summary = "List installable node types",
    description = "Returns the node types supported by the control plane and the field metadata needed to configure their type-specific settings.",
    responses(
        (status = 200, description = "Installable node types and their setting definitions.", body = NodeTypeDefinitionList),
        (status = 401, response = Unauthorized)
    )
)]
#[get("/node-types")]
pub async fn list_node_types() -> ApiResult<NodeTypeDefinitionList> {
    Ok(web::Json(node_type_catalog()))
}

#[cfg(test)]
mod tests {
    use actix_web::{App, http::StatusCode, test};
    use serde_json::Value;

    use super::*;

    #[actix_web::test]
    async fn endpoint_returns_the_supported_node_type_catalog() {
        let app = test::init_service(App::new().service(list_node_types)).await;
        let request = test::TestRequest::get().uri("/node-types").to_request();
        let response = test::call_service(&app, request).await;

        assert_eq!(response.status(), StatusCode::OK);
        let actual: Value = test::read_body_json(response).await;
        let expected: NodeTypeDefinitionList =
            serde_json::from_str(include_str!("../../../mockserver/api/v1/node-types.json"))
                .expect("node-types fixture should contain valid JSON");
        assert_eq!(
            actual,
            serde_json::to_value(expected).expect("node type catalog should serialize")
        );
    }

    #[actix_web::test]
    async fn catalog_total_matches_items_and_each_type_is_unique() {
        let catalog = node_type_catalog();

        assert_eq!(catalog.total, catalog.items.len() as u64);
        let mut types = catalog
            .items
            .iter()
            .map(|definition| serde_json::to_string(&definition.node_type).unwrap())
            .collect::<Vec<_>>();
        types.sort_unstable();
        types.dedup();
        assert_eq!(types.len(), catalog.items.len());
    }
}
