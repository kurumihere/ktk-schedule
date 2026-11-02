//! Optional local replay. The private archive is never embedded in the binary or committed.
use base64::Engine;
use ktk_schedule::{
    model::*,
    workspace::{Candidates, Site, Workspace},
};
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
    if !entries.iter().any(|e| {
        e["request"]["url"]
            .as_str()
            .and_then(|u| reqwest::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(|h| h == "workspace.ktk-45.ru"))
            .unwrap_or(false)
    }) {
        println!(
            "Skipping workspace replay: HAR archive does not contain workspace.ktk-45.ru entries"
        );
        return Ok(());
    }
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/sign-in"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;
    let mut mounted = HashSet::new();
    let (mut week, mut schedules, mut presets, mut halls, mut types, mut marks) =
        (0, 0, 0, 0, 0, 0);
    let mut schedule_body = None;
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
            if schedule_body.is_none() {
                schedule_body = Some(bytes.clone());
            }
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
    let site = Site::new(server.uri().parse()?, "test", Arc::new(Semaphore::new(12)));
    let client = Workspace::login(&site, "synthetic", "synthetic").await?;
    let week = Week {
        start: chrono::DateTime::from_timestamp_millis(week)
            .unwrap()
            .with_timezone(&chrono_tz::Asia::Yekaterinburg)
            .date_naive(),
        millis: week,
    };
    let candidates = client.discover(false).await?;
    assert!(!candidates.personal_schedules.is_empty());
    assert!(!candidates.group_schedules.is_empty());
    // Partial captures may lack group/teacher timetables (the browser never
    // opened them); answer those from the captured personal schedule so that
    // discovery is still verified end to end.
    if let Some(body) = schedule_body {
        let week_param = week.millis.to_string();
        for endpoint in candidates
            .group_schedules
            .iter()
            .chain(&candidates.teacher_schedules)
        {
            Mock::given(method("GET"))
                .and(path(endpoint.as_str()))
                .and(query_param("Week", week_param.clone()))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
                .mount(&server)
                .await;
        }
    }
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

#[tokio::test]
#[ignore = "requires KTK_HAR_PATH pointing to a private local HAR"]
async fn replay_captured_digital_environment_without_contacting_college() -> anyhow::Result<()> {
    let archive: Value = serde_json::from_slice(&std::fs::read(std::env::var("KTK_HAR_PATH")?)?)?;
    let entries = archive["log"]["entries"].as_array().unwrap();
    let mut verified_weeks = 0;
    let mut scripts = Candidates::default();
    let mut script_text = String::new();
    // Server functions the browser actually called, classified by their payload fields.
    let (mut sign_in, mut personal, mut group) = (None, None, None);
    for entry in entries {
        let url: reqwest::Url = entry["request"]["url"].as_str().unwrap().parse()?;
        if url.host_str() != Some("ktk.education-system.ru") || entry["response"]["status"] != 200 {
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
        if content["mimeType"]
            .as_str()
            .is_some_and(|m| m.contains("javascript"))
        {
            let text = String::from_utf8_lossy(&bytes);
            scripts.add(&text);
            script_text.push_str(&text);
            continue;
        }
        if url.path().starts_with("/_serverFn/") {
            let payload = url
                .query_pairs()
                .find(|(k, _)| k == "payload")
                .map(|(_, v)| v.into_owned())
                .or_else(|| {
                    entry["request"]["postData"]["text"]
                        .as_str()
                        .map(Into::into)
                });
            if let Some(payload) = payload {
                let request: Value = serde_json::from_str(&payload)?;
                let data = decode_seroval(&request["t"])?;
                let mut keys: Vec<_> = data["data"]
                    .as_object()
                    .map(|m| m.keys().map(String::as_str).collect())
                    .unwrap_or_default();
                keys.sort_unstable();
                let path = Some(url.path().to_owned());
                match keys.as_slice() {
                    ["collegeId", "fingerprint", "password"] => sign_in = path,
                    ["weekStart"] => personal = path,
                    ["groupId", "weekStart"] => group = path,
                    _ => (),
                }
            }
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => continue,
        };
        // Strings arrive escaped for JavaScript embedding (`<` as `\x3C`); none may survive decoding.
        assert!(
            !contains_seroval_escape(&decode_seroval(&value)?),
            "decoded server function response still contains seroval escapes"
        );
        if let Ok((days, refs)) = parse_digital_schedule(&value) {
            assert!(!days.is_empty(), "digital schedule parsed empty days");
            let total_subjects: usize = days.iter().map(|d| d.subjects.len()).sum();
            if total_subjects > 0 {
                assert!(!refs.halls.is_empty(), "scheduled week had empty halls");
            }
            assert!(parse_schedule(&value).is_ok());
            verified_weeks += 1;
        }
    }
    assert!(verified_weeks > 0, "no digital schedule weeks found in HAR");
    // Discovery must find exactly the functions the website itself called, whenever the
    // bundle that defines them was captured (cached bundles are missing from a HAR).
    let mut verified_functions = Vec::new();
    for (name, discovered, observed) in [
        ("sign-in", &scripts.digital_sign_in, &sign_in),
        (
            "personal schedule",
            &scripts.digital_student_schedule,
            &personal,
        ),
        ("group schedule", &scripts.digital_group_schedule, &group),
    ] {
        let Some(path) = observed else { continue };
        if script_text.contains(path.trim_start_matches("/_serverFn/")) {
            assert_eq!(discovered, observed, "{name} server function");
            verified_functions.push(name);
        }
    }
    println!(
        "Digital environment HAR replay passed: {verified_weeks} schedule weeks; observed sign-in={} personal={} group={}; discovery verified for {verified_functions:?}",
        sign_in.is_some(),
        personal.is_some(),
        group.is_some()
    );
    Ok(())
}

/// `\x3C` is seroval's escape for `<`; JSON itself never produces it.
fn contains_seroval_escape(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains("\\x3C"),
        Value::Array(items) => items.iter().any(contains_seroval_escape),
        Value::Object(map) => map.iter().any(|(k, v)| {
            contains_seroval_escape(&Value::String(k.clone())) || contains_seroval_escape(v)
        }),
        _ => false,
    }
}
