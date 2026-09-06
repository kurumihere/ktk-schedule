//! Optional local replay. The private archive is never embedded in the binary or committed.
use base64::Engine;
use ktk_schedule::{model::*, workspace::Workspace};
use serde_json::{Value, json};
use std::{collections::HashSet, sync::Arc};
use tokio::sync::Semaphore;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

#[tokio::test]
#[ignore = "requires KTK_HAR_PATH pointing to a private local HAR"]
async fn replay_captured_workspace_without_contacting_college() -> anyhow::Result<()> {
    let archive: Value = serde_json::from_slice(&std::fs::read(std::env::var("KTK_HAR_PATH")?)?)?;
    let entries = archive["log"]["entries"].as_array().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sign-in"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let mut mounted = HashSet::new();
    let (mut week, mut schedules, mut presets, mut halls, mut types, mut marks) =
        (0, 0, 0, 0, 0, 0);
    for entry in entries {
        let url: reqwest::Url = entry["request"]["url"].as_str().unwrap().parse()?;
        if url.host_str() != Some("workspace.ktk-45.ru") || entry["response"]["status"] != 200 {
            continue;
        }
        let content = &entry["response"]["content"];
        let Some(text) = content["text"].as_str() else {
            continue;
        };
        let bytes = if content["encoding"] == "base64" {
            base64::engine::general_purpose::STANDARD.decode(text)?
        } else {
            text.as_bytes().to_vec()
        };
        let queries: Vec<_> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        if let Some((_, value)) = queries.iter().find(|(k, _)| k == "Week") {
            week = value.parse()?;
            let value: Value = serde_json::from_slice(&bytes)?;
            // Do not print private payloads on failure.
            assert!(
                parse_schedule(&value).is_ok(),
                "captured schedule schema mismatch"
            );
            schedules += 1;
        }
        if url.path().ends_with("/call-preset") {
            presets = serde_json::from_slice::<Vec<Preset>>(&bytes)?.len();
        } else if url.path().ends_with("/pair-type") {
            types = serde_json::from_slice::<Vec<PairType>>(&bytes)?.len();
        } else if url.path().ends_with("/absence/mark") {
            marks = serde_json::from_slice::<Vec<Absence>>(&bytes)?.len();
        } else if url.path().ends_with("/lecture-hall") {
            let value: Value = serde_json::from_slice(&bytes)?;
            for list in value["LectureHalls"].as_object().unwrap().values() {
                halls += serde_json::from_value::<Vec<Hall>>(list.clone())?.len();
            }
        }
        if !mounted.insert(url.to_string()) {
            continue;
        }
        let mut mock = Mock::given(method("GET")).and(path(url.path()));
        for (key, value) in queries {
            mock = mock.and(query_param(key, value));
        }
        mock.respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(&server)
            .await;
    }
    assert!(schedules > 0 && week > 0);
    let client = Workspace::login(
        server.uri().parse()?,
        "test",
        "synthetic",
        "synthetic",
        Arc::new(Semaphore::new(12)),
    )
    .await?;
    let candidates = client.discover(false).await?;
    assert!(!candidates.personal_schedules.is_empty());
    assert!(!candidates.group_schedules.is_empty());
    assert!(
        !client
            .schedule(client.group, week, "personal")
            .await?
            .is_empty()
    );
    assert!(
        !client
            .schedule(client.group, week, &format!("group:{}", client.group))
            .await?
            .is_empty()
    );
    let references = client.references().await;
    assert_eq!(references.presets.len(), presets);
    for preset in references.presets.values() {
        assert_eq!(
            ktk_schedule::render::timings(preset).len(),
            preset.call_set.len()
        );
    }
    assert_eq!(references.halls.len(), halls);
    assert_eq!(references.types.len(), types);
    assert_eq!(references.absence.len(), marks);
    println!(
        "HAR replay passed: {schedules} schedule responses, {presets} call presets, {halls} halls, {types} pair types, {marks} absence marks"
    );
    Ok(())
}
