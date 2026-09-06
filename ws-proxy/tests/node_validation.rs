use actix_web::{App, http::StatusCode, test, web};
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;
use ws_proxy::api::models::{Host, HostStatus, Node};
use ws_proxy::api::{hosts::install_node_on_host, nodes::update_node};
use ws_proxy::registration::RegistrationServerMessage;
use ws_proxy::storage::{MemoryStore, Mutation, Store};

#[actix_web::test]
async fn submissions_validate_settings_before_mutating_or_dispatching() {
    let state: Store = std::sync::Arc::new(MemoryStore::default());
    let host_id = Uuid::new_v4();
    let session = Uuid::new_v4();
    state
        .mutate(Mutation::Register(
            Host {
                id: host_id,
                name: "host".into(),
                ip: "127.0.0.1".parse().unwrap(),
                version: "0.1.0".into(),
                status: HostStatus::Online,
                last_seen_at: Utc::now(),
                metadata: None,
            },
            session,
        ))
        .await
        .unwrap();
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(state.clone()))
            .service(install_node_on_host)
            .service(update_node),
    )
    .await;
    let create_uri = format!("/hosts/{host_id}/nodes");
    let cases = [
        (
            "audio_source",
            json!({"sampleRateHz":16000,"channels":1,"encoding":"pcm_s16le"}),
            vec![
                ("sampleRateHz", json!(12345)),
                ("channels", json!(0)),
                ("channels", json!(3)),
                ("encoding", json!("opus")),
                ("encoding", json!("pcm_f32le")),
                ("deviceName", json!("x".repeat(257))),
            ],
        ),
        (
            "transcriber",
            json!({"model":"large-v3","language":"en","beamSize":5}),
            vec![
                ("model", json!("unknown")),
                ("language", json!("en-US")),
                ("language", json!("EN")),
                ("language", json!("e")),
                ("beamSize", json!(0)),
                ("beamSize", json!(21)),
            ],
        ),
        (
            "translator",
            json!({"provider":"huggingface","sourceLanguage":"en","targetLanguage":"cs"}),
            vec![
                ("provider", json!("openai")),
                ("sourceLanguage", json!("en-US")),
                ("targetLanguage", json!("CS")),
            ],
        ),
        (
            "display",
            json!({"maxLines":3,"fontScale":1.0,"showOriginal":false}),
            vec![
                ("maxLines", json!(0)),
                ("maxLines", json!(11)),
                ("fontScale", json!(0.49)),
                ("fontScale", json!(4.01)),
            ],
        ),
    ];
    let mut upstream = None;
    for (kind, settings, invalid_fields) in cases {
        let valid = json!({"type":kind,"name":"test","enabled":true,"inputNodeId":upstream,"settings":settings});
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri(&create_uri)
                .set_json(&valid)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        let node: Node = test::read_body_json(response).await;
        let RegistrationServerMessage::ApplyNode {
            revision, update, ..
        } = state
            .snapshot()
            .await
            .unwrap()
            .commands
            .values()
            .filter(|c| c.node_id == node.id())
            .max_by_key(|c| c.revision)
            .unwrap()
            .message
            .clone()
        else {
            panic!("expected node creation dispatch");
        };
        assert_eq!(revision, 1);
        assert_eq!(update.input_node_id(), upstream);
        assert_eq!(
            state.snapshot().await.unwrap().nodes[&node.id()]
                .update
                .input_node_id(),
            upstream
        );
        upstream = Some(node.id());
        let mut invalid_input = valid.clone();
        invalid_input["inputNodeId"] = json!(Uuid::new_v4());
        let count = state.snapshot().await.unwrap().nodes.len();
        let commands_count = state.snapshot().await.unwrap().commands.len();
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri(&create_uri)
                .set_json(&invalid_input)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(state.snapshot().await.unwrap().nodes.len(), count);
        assert_eq!(
            state.snapshot().await.unwrap().commands.len(),
            commands_count
        );

        let update_uri = format!("/nodes/{}", node.id());
        let original = state.snapshot().await.unwrap().nodes[&node.id()].clone();
        let original_count = state.snapshot().await.unwrap().nodes.len();
        let mut invalid_requests: Vec<_> = invalid_fields
            .into_iter()
            .map(|(field, value)| {
                let mut request = valid.clone();
                request["settings"][field] = value;
                (request, format!("settings.{field}"))
            })
            .collect();
        for (field, value) in [
            ("name", json!("")),
            ("name", json!("x".repeat(129))),
            ("description", json!("x".repeat(1001))),
        ] {
            let mut request = valid.clone();
            request[field] = value;
            invalid_requests.push((request, field.to_owned()));
        }
        for (mut invalid, path) in invalid_requests {
            // Disabled submissions must also be validated.
            invalid["enabled"] = json!(false);
            invalid["inputNodeId"] = Value::Null;
            for request in [
                test::TestRequest::post().uri(&create_uri),
                test::TestRequest::put().uri(&update_uri),
            ] {
                let response =
                    test::call_service(&app, request.set_json(&invalid).to_request()).await;
                assert_eq!(
                    response.status(),
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "{invalid}"
                );
                assert_eq!(
                    response.headers().get("content-type").unwrap(),
                    "application/problem+json"
                );
                let problem: Value = test::read_body_json(response).await;
                assert_eq!(problem["errors"][0]["path"], path);

                assert_eq!(
                    state.snapshot().await.unwrap().commands.len(),
                    commands_count
                );
                let registry = state.snapshot().await.unwrap().nodes;
                assert_eq!(registry.len(), original_count);
                assert_eq!(registry[&node.id()].revision, original.revision);
                assert_eq!(registry[&node.id()].update, original.update);
                assert_eq!(
                    serde_json::to_value(&registry[&node.id()].node).unwrap(),
                    serde_json::to_value(&original.node).unwrap()
                );
            }
        }
        let mut valid_update = valid;
        valid_update["inputNodeId"] = Value::Null;
        valid_update["name"] = json!("updated");
        let response = test::call_service(
            &app,
            test::TestRequest::put()
                .uri(&update_uri)
                .set_json(&valid_update)
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(matches!(
            state
                .snapshot()
                .await
                .unwrap()
                .commands
                .values()
                .filter(|c| c.node_id == node.id())
                .max_by_key(|c| c.revision)
                .unwrap()
                .message
                .clone(),
            RegistrationServerMessage::ApplyNode { revision: 2, .. }
        ));
    }
}

#[test]
async fn numeric_boundaries_are_accepted_and_nonfinite_scales_rejected() {
    use validator::Validate;
    use ws_proxy::api::models::{AudioSourceSettings, DisplaySettings, TranscriberSettings};
    for rate in [8000, 16000, 22050, 44100, 48000] {
        for channels in [1, 2] {
            let settings: AudioSourceSettings = serde_json::from_value(
                json!({"sampleRateHz":rate,"channels":channels,"encoding":"pcm_s16le"}),
            )
            .unwrap();
            assert!(settings.validate().is_ok());
        }
    }
    for beam in [1, 20] {
        let settings: TranscriberSettings =
            serde_json::from_value(json!({"model":"tiny","language":"eng","beamSize":beam}))
                .unwrap();
        assert!(settings.validate().is_ok());
    }
    for lines in [1, 10] {
        for scale in [0.5, 4.0] {
            let mut settings: DisplaySettings = serde_json::from_value(
                json!({"maxLines":lines,"fontScale":scale,"showOriginal":false}),
            )
            .unwrap();
            assert!(settings.validate().is_ok());
            for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                settings.font_scale = invalid;
                assert!(settings.validate().is_err());
            }
        }
    }
}
