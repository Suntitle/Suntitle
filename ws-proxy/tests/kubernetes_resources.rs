use kube::CustomResourceExt;
use serde_json::json;
use ws_proxy::kubernetes::resources::{SuntitleHost, SuntitleNode, SuntitleNodeSpec, crd_yaml};

#[test]
fn checked_in_manifests_match_the_generator() {
    assert_eq!(
        crd_yaml().unwrap(),
        include_str!("../../kubernetes/crds.yaml")
    );
}

#[test]
fn resources_have_status_subresources_and_conditions_map() {
    for crd in [SuntitleHost::crd(), SuntitleNode::crd()] {
        let version = &crd.spec.versions[0];
        assert!(version.subresources.as_ref().unwrap().status.is_some());
        assert_eq!(crd.spec.scope, "Namespaced");
        let schema = serde_json::to_value(version.schema.as_ref().unwrap()).unwrap();
        let conditions =
            &schema["openAPIV3Schema"]["properties"]["status"]["properties"]["conditions"];
        assert_eq!(conditions["x-kubernetes-list-type"], "map");
        assert_eq!(conditions["x-kubernetes-list-map-keys"], json!(["type"]));
    }
}

#[test]
fn spec_uses_typed_settings_and_nullable_references() {
    let document = json!({"name":"Mic", "hostRef":{"name":"studio"},
        "settings":{"audio_source":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le"}}});
    let spec: SuntitleNodeSpec = serde_json::from_value(document.clone()).unwrap();
    assert!(spec.enabled);
    assert!(spec.input_ref.is_none());
    let serialized = serde_json::to_value(spec).unwrap();
    assert!(serialized.get("type").is_none());
    assert_eq!(
        serialized["settings"]["audio_source"]["sampleRateHz"],
        16000
    );
    let mut invalid = document;
    invalid["settings"]["audio_source"]["channels"] = json!("not a number");
    assert!(serde_json::from_value::<SuntitleNodeSpec>(invalid).is_err());
}

#[test]
fn generation_command_does_not_initialize_storage_or_kubernetes() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_control_api"))
        .arg("generate-crds")
        .env("DATABASE_URL", "invalid")
        .env("SUNTITLE_BOOTSTRAP_KUBECONFIG", "/does-not-exist")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        crd_yaml().unwrap()
    );
}

#[test]
fn derived_cel_validates_type_settings_and_immutable_fields_locally() {
    let valid = json!({"name":"Mic", "hostRef":{"name":"studio"}, "enabled":true,
        "settings":{"audio_source":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le"}}});
    SuntitleNodeSpec::validate_cel(&valid, None).unwrap();
    let resource = SuntitleNode::new("mic", serde_json::from_value(valid.clone()).unwrap());
    resource.validate_cel().unwrap();

    let mut invalid = valid.clone();
    invalid["settings"] =
        json!({"display":{"maxLines":3,"fontScale":1.0,"showOriginal":true,"theme":"dark"}});
    SuntitleNodeSpec::validate_cel(&invalid, None).unwrap();
    assert!(SuntitleNodeSpec::validate_cel(&invalid, Some(&valid)).is_err());
    let mut editable = valid.clone();
    editable["settings"]["audio_source"]["channels"] = json!(2);
    SuntitleNodeSpec::validate_cel(&editable, Some(&valid)).unwrap();
    invalid = valid.clone();
    invalid["hostRef"]["name"] = json!("replacement");
    assert!(SuntitleNodeSpec::validate_cel(&invalid, Some(&valid)).is_err());
    let changed = SuntitleNode::new("mic", serde_json::from_value(invalid).unwrap());
    assert!(changed.validate_cel_update(&resource).is_err());
}

#[test]
fn field_cel_validates_each_settings_variant() {
    let variants = [
        (
            "audio_source",
            json!({"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le","deviceName":null}),
            vec![
                ("sampleRateHz", json!(12345)),
                ("channels", json!(0)),
                ("channels", json!(3)),
                ("encoding", json!("opus")),
            ],
        ),
        (
            "transcriber",
            json!({"model":"small","language":"en","task":"transcribe","beamSize":5,"vadEnabled":true}),
            vec![
                ("language", json!("en-US")),
                ("task", json!("unknown")),
                ("beamSize", json!(0)),
                ("beamSize", json!(21)),
                ("model", json!("unknown")),
                ("model", json!(null)),
            ],
        ),
        (
            "translator",
            json!({"sourceLanguage":"en","targetLanguage":"cs","provider":"huggingface","model":null,"preserveFormatting":true}),
            vec![
                ("sourceLanguage", json!("EN")),
                ("targetLanguage", json!("en-US")),
                ("provider", json!("openai")),
            ],
        ),
        (
            "display",
            json!({"maxLines":3,"fontScale":1.0,"showOriginal":true,"theme":"dark"}),
            vec![
                ("maxLines", json!(0)),
                ("maxLines", json!(11)),
                ("fontScale", json!(0.4)),
                ("fontScale", json!(4.1)),
                ("theme", json!("neon")),
            ],
        ),
    ];
    for (node_type, settings, invalid_fields) in variants {
        let valid = json!({"name":"Node", "hostRef":{"name":"studio"}, "enabled":true,
            "settings":{(node_type):settings}});
        SuntitleNodeSpec::validate_cel(&valid, None).unwrap();
        for (field, value) in invalid_fields {
            let mut invalid = valid.clone();
            invalid["settings"][node_type][field] = value;
            assert!(
                SuntitleNodeSpec::validate_cel(&invalid, None).is_err(),
                "accepted invalid {node_type}.{field}: {}",
                invalid["settings"][node_type][field]
            );
        }
    }
}

#[test]
fn translator_model_remains_optional_and_unrestricted_by_transcriber_rules() {
    let mut valid = json!({"name":"Translator", "hostRef":{"name":"studio"}, "enabled":true,
        "settings":{"translator":{"sourceLanguage":"en","targetLanguage":"cs",
            "provider":"huggingface","preserveFormatting":true}}});
    SuntitleNodeSpec::validate_cel(&valid, None).unwrap();
    for model in [json!(null), json!("custom-translator-model")] {
        valid["settings"]["translator"]["model"] = model;
        SuntitleNodeSpec::validate_cel(&valid, None).unwrap();
    }
}

#[test]
fn settings_reject_missing_unknown_multiple_and_mismatched_variants() {
    let audio = json!({"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le"});
    let display = json!({"maxLines":3,"fontScale":1.0,"showOriginal":true});
    for settings in [
        json!({}),
        json!({"unknown":audio}),
        json!({"audio_source":audio,"display":display}),
        json!({"transcriber":audio}),
        json!({"audio_source":null}),
        audio,
    ] {
        let document = json!({"name":"Node","hostRef":{"name":"studio"},"settings":settings});
        assert!(
            serde_json::from_value::<SuntitleNodeSpec>(document.clone()).is_err(),
            "accepted invalid settings: {document}"
        );
    }
}

#[test]
fn variant_schemas_keep_models_and_defaults_independent() {
    let crd = SuntitleNode::crd();
    let schema = serde_json::to_value(crd.spec.versions[0].schema.as_ref().unwrap()).unwrap();
    let spec = &schema["openAPIV3Schema"]["properties"]["spec"];
    assert!(spec["properties"].get("type").is_none());
    let variants = &spec["properties"]["settings"]["properties"];
    let transcriber = &variants["transcriber"];
    assert_eq!(transcriber["properties"]["model"]["type"], "string");
    assert_ne!(transcriber["properties"]["model"]["nullable"], true);
    assert!(
        transcriber["required"]
            .as_array()
            .unwrap()
            .contains(&json!("model"))
    );
    assert_eq!(
        variants["translator"]["properties"]["model"]["nullable"],
        true
    );
    for (variant, field, default) in [
        ("transcriber", "task", json!("transcribe")),
        ("transcriber", "beamSize", json!(5)),
        ("transcriber", "vadEnabled", json!(true)),
        ("translator", "preserveFormatting", json!(true)),
        ("display", "theme", json!("dark")),
    ] {
        assert_eq!(variants[variant]["properties"][field]["default"], default);
    }
}

#[test]
fn phase_two_api_fixtures_use_resource_contracts() {
    use ws_proxy::kubernetes::store::{HostList, NodeCreate, NodeList, NodeUpdate};
    let hosts: HostList = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/hosts.json"
    ))
    .unwrap();
    let nodes: NodeList = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/nodes.json"
    ))
    .unwrap();
    assert_eq!(hosts.items.len(), 1);
    assert_eq!(nodes.items.len(), 4);
    for node in &nodes.items {
        assert_eq!(
            node.spec.host_ref.name,
            hosts.items[0].metadata.name.as_ref().unwrap().as_str()
        );
        assert!(node.status.is_none());
    }
    let create: NodeCreate = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/create.json"
    ))
    .unwrap();
    let update: NodeUpdate = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/update.json"
    ))
    .unwrap();
    assert_eq!(create.name, nodes.items[0].metadata.name);
    assert_eq!(update.spec.name, create.spec.name);
}
