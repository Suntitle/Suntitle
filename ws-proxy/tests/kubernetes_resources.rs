use kube::CustomResourceExt;
use serde_json::json;
use ws_proxy::kubernetes::resources::{
    SuntitleExtension, SuntitleHost, SuntitleNode, SuntitleNodeSpec, crd_yaml,
};

#[test]
fn checked_in_manifests_match_the_generator() {
    assert_eq!(
        crd_yaml().unwrap(),
        include_str!("../../kubernetes/crds.yaml")
    );
}

#[test]
fn resources_have_status_subresources_and_conditions_map() {
    for crd in [
        SuntitleHost::crd(),
        SuntitleNode::crd(),
        SuntitleExtension::crd(),
    ] {
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
    use ws_proxy::kubernetes::store::{NodeCreate, NodeUpdate, ResourceList};
    let hosts: ResourceList<SuntitleHost> = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/hosts.json"
    ))
    .unwrap();
    let nodes: ResourceList<SuntitleNode> = serde_json::from_str(include_str!(
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

#[test]
fn extension_schema_preserves_adapter_and_constrains_source_fields() {
    let crd = SuntitleExtension::crd();
    let schema = serde_json::to_value(crd.spec.versions[0].schema.as_ref().unwrap()).unwrap();
    let spec = &schema["openAPIV3Schema"]["properties"]["spec"];
    assert_eq!(
        spec["properties"]["sourceType"]["enum"],
        json!(["local", "git", "oci", "http"])
    );
    assert_eq!(spec["properties"]["source"]["minLength"], 1);
    assert_eq!(spec["properties"]["adapter"]["type"], "object");
    let adapter_schema = &spec["properties"]["adapter"]["properties"];
    assert_eq!(
        adapter_schema["type"]["enum"],
        json!(["audio_source", "transcriber", "translator", "display"])
    );
    let execution = &adapter_schema["execution"];
    assert_eq!(execution["type"], "object");
    assert!(
        execution
            .get("x-kubernetes-preserve-unknown-fields")
            .is_none()
    );
    assert_eq!(
        execution["properties"]["runtime"]["enum"],
        json!(["executable", "python", "pythonModule"])
    );
    assert_eq!(execution["properties"]["runtime"]["default"], "executable");
    assert_eq!(execution["properties"]["command"]["type"], "string");
    assert_eq!(execution["required"], json!(["command"]));
    assert_eq!(execution["properties"]["command"]["minLength"], 1);
    for old in ["executable", "script", "module"] {
        assert!(execution["properties"].get(old).is_none());
    }
    let node_schema = serde_json::to_value(SuntitleNode::crd()).unwrap();
    let resolved_execution = &node_schema["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]
        ["status"]["properties"]["resolvedAdapter"]["properties"]["execution"];
    assert_eq!(
        resolved_execution, execution,
        "declared and resolved adapters MUST reuse the execution schema"
    );
    use utoipa::OpenApi;
    let api = serde_json::to_value(ws_proxy::api::ApiDoc::openapi()).unwrap();
    assert_eq!(
        api["components"]["schemas"]["SuntitleExtensionSpec"]["properties"]["adapter"]["properties"]
            ["execution"]["properties"]["runtime"]["enum"],
        json!(["executable", "python", "pythonModule"])
    );
    let setting = &adapter_schema["settings"]["additionalProperties"];
    assert_eq!(setting["properties"]["required"]["type"], "boolean");
    assert_eq!(setting["properties"]["required"]["default"], false);
    assert_eq!(
        setting["properties"]["default"]["x-kubernetes-preserve-unknown-fields"],
        true
    );
    assert_eq!(setting["properties"]["default"]["nullable"], true);
    let binding = &adapter_schema["arguments"]["items"];
    assert_eq!(
        binding["properties"]["mode"]["enum"],
        json!(["value", "switch"])
    );
    assert_eq!(binding["properties"]["mode"]["default"], "value");
    assert_eq!(binding["required"], json!(["source"]));
    let adapter = json!({"type":"transcriber", "execution":{"runtime":"executable", "command":"./run"},
        "settings":{"nested":{"default":{"values":[null, false, 0, ""]}}},
        "arguments":[{"flag":"--server", "source":"runtime.websocketUrl"}]});
    for source_type in ["local", "git", "oci", "http"] {
        let document = json!({"apiVersion":"control.suntitle.io/v1alpha1", "kind":"SuntitleExtension",
            "metadata":{"name":"custom-transcriber"}, "spec":{"name":"Custom transcriber",
            "sourceType":source_type, "source":"example", "adapter":adapter}});
        let extension: SuntitleExtension = serde_json::from_value(document).unwrap();
        assert_eq!(
            serde_json::to_value(extension).unwrap()["spec"]["adapter"],
            serde_json::to_value(
                serde_json::from_value::<ws_proxy::kubernetes::adapter::ExtensionAdapter>(
                    adapter.clone()
                )
                .unwrap()
            )
            .unwrap()
        );
    }
}

#[test]
fn node_extension_reference_is_optional_and_uses_resource_ref_schema() {
    let mut document = json!({"name":"Mic", "hostRef":{"name":"studio"},
        "settings":{"audio_source":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le"}}});
    let spec: SuntitleNodeSpec = serde_json::from_value(document.clone()).unwrap();
    assert!(spec.extension_ref.is_none());
    assert!(
        serde_json::to_value(spec)
            .unwrap()
            .get("extensionRef")
            .is_none()
    );
    document["extensionRef"] = json!({"name":"custom-source"});
    document["settings"] = json!({"custom":{}});
    let spec: SuntitleNodeSpec = serde_json::from_value(document).unwrap();
    assert_eq!(spec.extension_ref.unwrap().name, "custom-source");
    let crd = SuntitleNode::crd();
    let schema = serde_json::to_value(crd.spec.versions[0].schema.as_ref().unwrap()).unwrap();
    let properties = &schema["openAPIV3Schema"]["properties"]["spec"]["properties"];
    assert_eq!(
        properties["extensionRef"]["properties"],
        properties["inputRef"]["properties"]
    );
}

#[test]
fn custom_settings_require_extension_ref_and_preserve_arbitrary_values() {
    let settings = json!({"custom":{"nested":{"values":[null, false, 0, "", [1], {"key":"value"}]},
        "model":"unrestricted-model", "language":"unrestricted-language"}});
    let mut custom = json!({"name":"Custom", "hostRef":{"name":"studio"}, "enabled":true,
        "extensionRef":{"name":"plugin"}, "settings":settings});
    SuntitleNodeSpec::validate_cel(&custom, None).unwrap();
    let parsed: SuntitleNodeSpec = serde_json::from_value(custom.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap()["settings"], settings);
    let crd = SuntitleNode::crd();
    let schema = serde_json::to_value(crd.spec.versions[0].schema.as_ref().unwrap()).unwrap();
    let custom_schema = &schema["openAPIV3Schema"]["properties"]["spec"]["properties"]["settings"]
        ["properties"]["custom"];
    assert_eq!(custom_schema["type"], "object");
    assert_eq!(custom_schema["x-kubernetes-preserve-unknown-fields"], true);

    custom["settings"] = json!({"custom":{}});
    SuntitleNodeSpec::validate_cel(&custom, None).unwrap();
    for reference in [None, Some(json!(null))] {
        let mut invalid = custom.clone();
        invalid.as_object_mut().unwrap().remove("extensionRef");
        if let Some(reference) = reference {
            invalid["extensionRef"] = reference;
        }
        assert!(SuntitleNodeSpec::validate_cel(&invalid, None).is_err());
    }
    for settings in [
        json!({"custom":null}),
        json!({"custom":[]}),
        json!({"custom":{},"display":{"maxLines":3,"fontScale":1.0,"showOriginal":true}}),
    ] {
        let mut invalid = custom.clone();
        invalid["settings"] = settings;
        assert!(serde_json::from_value::<SuntitleNodeSpec>(invalid).is_err());
    }
}

#[test]
fn built_in_settings_forbid_extension_ref_and_cannot_change_to_custom() {
    for settings in [
        json!({"audio_source":{"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le"}}),
        json!({"transcriber":{"model":"small","language":"en"}}),
        json!({"translator":{"sourceLanguage":"en","targetLanguage":"cs","provider":"huggingface"}}),
        json!({"display":{"maxLines":3,"fontScale":1.0,"showOriginal":true}}),
    ] {
        let mut builtin = json!({"name":"Built-in", "hostRef":{"name":"studio"}, "enabled":true, "settings":settings});
        // Deserialize to include defaults used by CEL rules.
        builtin =
            serde_json::to_value(serde_json::from_value::<SuntitleNodeSpec>(builtin).unwrap())
                .unwrap();
        SuntitleNodeSpec::validate_cel(&builtin, None).unwrap();
        let mut invalid = builtin.clone();
        invalid["extensionRef"] = json!({"name":"plugin"});
        assert!(SuntitleNodeSpec::validate_cel(&invalid, None).is_err());
        let custom = json!({"name":"Custom", "hostRef":{"name":"studio"}, "enabled":true,
            "extensionRef":{"name":"plugin"}, "settings":{"custom":{}}});
        assert!(SuntitleNodeSpec::validate_cel(&custom, Some(&builtin)).is_err());
        assert!(SuntitleNodeSpec::validate_cel(&builtin, Some(&custom)).is_err());
        let mut changed = custom.clone();
        changed["extensionRef"]["name"] = json!("another-plugin");
        changed["settings"]["custom"] = json!({"threads":4});
        SuntitleNodeSpec::validate_cel(&changed, Some(&custom)).unwrap();
    }
}

#[test]
fn extension_api_fixtures_match_resource_contracts() {
    use ws_proxy::kubernetes::store::{ExtensionCreate, ResourceList};
    let create: ExtensionCreate = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/extension-create.json"
    ))
    .unwrap();
    let extension: SuntitleExtension = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/extension.json"
    ))
    .unwrap();
    let list: ResourceList<SuntitleExtension> = serde_json::from_str(include_str!(
        "../../mockserver/fixtures/kubernetes/extensions.json"
    ))
    .unwrap();
    assert_eq!(create.name, extension.metadata.name);
    assert_eq!(
        serde_json::to_value(create.spec).unwrap(),
        serde_json::to_value(extension.spec).unwrap()
    );
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.resource_version, "10");
}

/// Runnable equality includes immutable fields and excludes metadata and pipeline inputs.
#[test]
fn node_spec_equality_compares_typed_runnable_configuration() {
    let document = json!({
        "name":"Custom", "hostRef":{"name":"host"}, "extensionRef":{"name":"extension"},
        "settings":{"custom":{"language":"en"}}
    });
    let spec: SuntitleNodeSpec = serde_json::from_value(document.clone()).unwrap();
    let mut metadata = spec.clone();
    metadata.name = "Renamed".into();
    metadata.description = Some("Updated description".into());
    metadata.input_ref = Some(serde_json::from_value(json!({"name":"upstream"})).unwrap());
    assert_eq!(spec, metadata);
    for change in [
        json!({"hostRef":{"name":"other-host"}}),
        json!({"enabled":false}),
        json!({"settings":{"custom":{"language":"cs"}}}),
        json!({"extensionRef":{"name":"other-extension"}}),
        json!({"restartToken":"restart"}),
        json!({"settings":{"display":{"maxLines":3,"fontScale":1.0,"showOriginal":false}}}),
    ] {
        let mut changed = document.clone();
        changed
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        let changed: SuntitleNodeSpec = serde_json::from_value(changed).unwrap();
        assert_ne!(spec, changed);
    }
}
