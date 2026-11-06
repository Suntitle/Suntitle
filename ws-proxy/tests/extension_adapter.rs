use serde_json::{Value, json};
use suntitle_core::kubernetes::{
    adapter::{ExtensionAdapter, MissingRequiredSettings},
    resources::{SuntitleExtensionSpec, SuntitleNodeSpec},
};

fn adapter() -> ExtensionAdapter {
    serde_json::from_value(json!({
        "type":"transcriber",
        "execution":{"runtime":"executable", "command":"./bin/transcriber"},
        "settings":{
            "model":{"default":"fast"},
            "threads":{"default":4},
            "useGpu":{"default":true},
            "language":{"required":true},
            "device":{"required":false}
        },
        "arguments":[
            {"flag":"--server", "source":"runtime.websocketUrl"},
            {"flag":"--model", "source":"settings.model"},
            {"flag":"--threads", "source":"settings.threads"},
            {"flag":"--gpu", "source":"settings.useGpu", "mode":"switch"},
            {"flag":"--language", "source":"settings.language"},
            {"flag":"--device", "source":"settings.device"}
        ]
    }))
    .unwrap()
}

#[test]
fn nodes_inherit_extension_defaults_and_mappings_without_changing_desired_state() {
    let extension: SuntitleExtensionSpec = serde_json::from_value(json!({
        "name":"Transcriber", "sourceType":"local", "source":"/extensions/example", "adapter":adapter()
    })).unwrap();
    let before = serde_json::to_value(&extension).unwrap();
    for (language, model) in [("en", "small"), ("cs", "custom model")] {
        let node: SuntitleNodeSpec = serde_json::from_value(json!({
            "name":"Node", "hostRef":{"name":"host"}, "extensionRef":{"name":"extension"},
            "settings":{"custom":{"language":language,"model":model,"extra":{"nested":[1,2]}}}
        }))
        .unwrap();
        let overrides = serde_json::to_value(&node).unwrap();
        let resolved = node.resolve_extension(&extension).unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(&resolved.settings).unwrap(),
            json!({
                "language":language, "model":model, "threads":4,"useGpu":true,"extra":{"nested":[1,2]}
            })
        );
        assert_eq!(resolved.execution, extension.adapter.execution);
        assert_eq!(resolved.node_type, extension.adapter.node_type);
        assert_eq!(resolved.arguments, extension.adapter.arguments[..5]);
        assert_eq!(serde_json::to_value(node).unwrap(), overrides);
    }
    assert_eq!(serde_json::to_value(extension).unwrap(), before);
}

#[test]
fn required_without_defaults_produces_an_execution_resolution_error() {
    let mut adapter = adapter();
    adapter.settings.insert(
        "explicit".into(),
        serde_json::from_value(json!({"required":true})).unwrap(),
    );
    let error = adapter.resolve(&Default::default()).unwrap_err();
    assert_eq!(
        error,
        MissingRequiredSettings(vec!["explicit".into(), "language".into()])
    );
    assert_eq!(
        error.to_string(),
        "missing required extension settings: explicit, language"
    );
    let overrides = serde_json::from_value(json!({"explicit":null,"language":""})).unwrap();
    assert!(adapter.resolve(&overrides).is_ok());
}

#[test]
fn default_presence_and_override_presence_preserve_every_json_value() {
    for value in [
        Value::Null,
        json!(false),
        json!(0),
        json!(""),
        json!([]),
        json!({"nested":[null]}),
    ] {
        let adapter: ExtensionAdapter = serde_json::from_value(json!({
            "type":"display", "execution":{"command":"./run"}, "settings":{"value":{"default":value}},
            "arguments":[{"source":"settings.value"}]
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&adapter).unwrap()["settings"]["value"]["default"],
            value
        );
        assert!(
            serde_json::to_value(&adapter).unwrap()["settings"]["value"]
                .get("default")
                .is_some()
        );
        assert_eq!(
            adapter.resolve(&Default::default()).unwrap().settings["value"],
            value
        );
        let overrides = serde_json::from_value(json!({"value":value})).unwrap();
        assert_eq!(
            adapter.resolve(&overrides).unwrap().settings["value"],
            value
        );
    }
    let adapter = adapter();
    for value in [Value::Null, json!(false), json!(0), json!("")] {
        let overrides = serde_json::from_value(json!({"language":"en","threads":value})).unwrap();
        assert_eq!(
            adapter.resolve(&overrides).unwrap().settings["threads"],
            value
        );
    }
}

#[test]
fn optional_settings_omit_their_binding_only_when_absent_and_still_use_defaults() {
    let mut adapter = adapter();
    let without = serde_json::from_value(json!({"language":"en"})).unwrap();
    assert_eq!(adapter.resolve(&without).unwrap().arguments.len(), 5);
    for value in [Value::Null, json!(false), json!(0), json!("")] {
        let overrides = serde_json::from_value(json!({"language":"en","device":value})).unwrap();
        let resolved = adapter.resolve(&overrides).unwrap();
        assert_eq!(resolved.arguments, adapter.arguments);
        assert_eq!(resolved.settings["device"], value);
    }
    adapter.settings.get_mut("device").unwrap().default = Some(json!("default device"));
    let resolved = adapter.resolve(&without).unwrap();
    assert_eq!(resolved.arguments, adapter.arguments);
    assert_eq!(resolved.settings["device"], "default device");
}

#[test]
fn switch_requires_a_flag_in_the_crd_rule() {
    use suntitle_core::kubernetes::adapter::ArgumentBinding;
    let valid = json!({"flag":"--gpu", "source":"settings.useGpu", "mode":"switch"});
    ArgumentBinding::validate_cel(&valid, None).unwrap();
    for flag in [None, Some(Value::Null)] {
        let mut invalid = valid.clone();
        invalid.as_object_mut().unwrap().remove("flag");
        if let Some(flag) = flag {
            invalid["flag"] = flag;
        }
        assert!(ArgumentBinding::validate_cel(&invalid, None).is_err());
    }
    ArgumentBinding::validate_cel(&json!({"source":"settings.model", "mode":"value"}), None)
        .unwrap();
}

#[test]
fn settings_are_optional_unless_explicitly_required_and_defaults_supply_values() {
    let adapter: ExtensionAdapter = serde_json::from_value(json!({
        "type":"display", "execution":{"command":"./run"},
        "settings":{
            "optional":{}, "defaulted":{"default":"value"},
            "requiredWithDefault":{"required":true,"default":null}
        },
        "arguments":[{"source":"settings.optional"},{"source":"settings.defaulted"},
            {"source":"settings.requiredWithDefault"}]
    }))
    .unwrap();
    assert!(!adapter.settings["optional"].required);
    assert!(!adapter.settings["defaulted"].required);
    let resolved = adapter.resolve(&Default::default()).unwrap();
    assert!(!resolved.settings.contains_key("optional"));
    assert_eq!(resolved.settings["defaulted"], "value");
    assert_eq!(resolved.settings["requiredWithDefault"], Value::Null);
    assert_eq!(resolved.arguments, adapter.arguments[1..]);
    let serialized = serde_json::to_value(&adapter).unwrap();
    assert_eq!(
        serialized["settings"]["optional"],
        json!({"required":false})
    );
    assert_eq!(
        serialized["settings"]["defaulted"],
        json!({"default":"value", "required":false})
    );
    assert_eq!(
        serialized["settings"]["requiredWithDefault"]["required"],
        true
    );
    let roundtrip: ExtensionAdapter = serde_json::from_value(serialized).unwrap();
    assert!(roundtrip.settings["requiredWithDefault"].required);
}

/// Standard binding sources are typed while setting keys and values remain user-owned.
#[test]
fn argument_sources_reuse_the_wire_contract_and_reject_unknown_runtime_fields() {
    use suntitle_core::kubernetes::adapter::{ArgumentSource, RuntimeValue};
    for (wire, expected) in [
        (
            "runtime.websocketUrl",
            ArgumentSource::Runtime(RuntimeValue::WebsocketUrl),
        ),
        (
            "settings.custom.key",
            ArgumentSource::Setting("custom.key".into()),
        ),
    ] {
        let source: ArgumentSource = serde_json::from_value(json!(wire)).unwrap();
        assert_eq!(source, expected);
        assert_eq!(serde_json::to_value(source).unwrap(), json!(wire));
    }
    for wire in ["runtime.unknown", "settings.", "websocketUrl", ""] {
        assert!(serde_json::from_value::<ArgumentSource>(json!(wire)).is_err());
    }
}
