use anyhow::{Result, bail, ensure};
use chrono::{DateTime, Datelike, Days, NaiveDate, TimeZone};
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// Bound both persisted attachment lists and the work done by a single command.
pub const MAX_ATTACHMENTS: usize = 64;
const MAX_ATTACHMENT_NODES: usize = 4096;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "PascalCase")]
pub struct ScheduleDay {
    #[serde(deserialize_with = "null_default")]
    pub call_preset: i64,
    pub date: String,
    #[serde(deserialize_with = "null_default")]
    pub today: bool,
    #[serde(deserialize_with = "null_default")]
    pub max_pair: i64,
    #[serde(deserialize_with = "null_default")]
    pub subjects: Vec<Subject>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "PascalCase")]
pub struct Subject {
    #[serde(deserialize_with = "null_default")]
    pub appraisal: i64,
    #[serde(deserialize_with = "null_default")]
    pub discipline: String,
    #[serde(deserialize_with = "null_default")]
    pub lecture_hall: i64,
    #[serde(deserialize_with = "null_default")]
    pub mark: i64,
    #[serde(deserialize_with = "null_default")]
    pub pair: i64,
    #[serde(deserialize_with = "null_default")]
    pub subgroup: String,
    #[serde(deserialize_with = "null_default")]
    pub teacher: String,
    #[serde(deserialize_with = "null_default")]
    pub group: String,
    #[serde(deserialize_with = "null_default")]
    pub call_preset: i64,
    #[serde(deserialize_with = "null_default")]
    pub extended_data: ExtendedData,
    #[serde(deserialize_with = "null_default")]
    pub extra_data: ExtraData,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "PascalCase")]
pub struct ExtendedData {
    #[serde(deserialize_with = "null_default")]
    pub academic_hour: i64,
    #[serde(deserialize_with = "null_default")]
    pub discipline_full: String,
    #[serde(deserialize_with = "null_default")]
    pub pair_type: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "PascalCase")]
pub struct ExtraData {
    #[serde(deserialize_with = "null_default")]
    pub lecture_theme: String,
    #[serde(deserialize_with = "null_default")]
    pub lecture_homework: String,
    #[serde(deserialize_with = "null_default")]
    pub lecture_type: i64,
    #[serde(deserialize_with = "null_default")]
    pub sheet: i64,
    #[serde(deserialize_with = "null_default")]
    pub homework: Homework,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct Homework {
    pub task: Option<String>,
    pub deadline: Option<String>,
    pub webinar: Option<String>,
    pub files: Vec<i64>,
    pub lock_upload: Option<bool>,
}

pub fn null_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

impl<'de> Deserialize<'de> for Homework {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let v = Value::deserialize(deserializer)?;
        Ok(Self {
            task: v["Task"].as_str().map(Into::into),
            deadline: v["Deadline"].as_str().map(Into::into),
            webinar: v["Webinar"].as_str().map(Into::into),
            lock_upload: v["LockUpload"].as_bool(),
            files: attachment_ids(&v).map_err(serde::de::Error::custom)?,
        })
    }
}

pub fn positive_id(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str()?.trim().parse().ok())
        .filter(|n| *n > 0)
}

pub fn attachment_ids(value: &Value) -> Result<Vec<i64>> {
    fn collect(
        v: &Value,
        out: &mut Vec<i64>,
        seen: &mut HashSet<i64>,
        nodes: &mut usize,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth <= 32, "attachment nesting exceeds limit");
        ensure!(
            *nodes < MAX_ATTACHMENT_NODES,
            "attachment structure exceeds limit"
        );
        *nodes += 1;
        match v {
            Value::Array(values) => {
                for v in values {
                    collect(v, out, seen, nodes, depth + 1)?;
                }
            }
            Value::Object(map) => {
                for key in [
                    "ID",
                    "Id",
                    "id",
                    "FileID",
                    "fileID",
                    "fileId",
                    "DocumentID",
                    "documentID",
                    "documentId",
                    "DocID",
                    "docID",
                    "UserFileID",
                    "File",
                    "file",
                    "Document",
                    "document",
                    "Attachment",
                    "attachment",
                    "UserFile",
                    "userFile",
                ] {
                    if let Some(v) = map.get(key) {
                        collect(v, out, seen, nodes, depth + 1)?;
                    }
                }
            }
            _ => {
                if let Some(id) = positive_id(v)
                    && seen.insert(id)
                {
                    ensure!(
                        out.len() < MAX_ATTACHMENTS,
                        "too many attachments (maximum {MAX_ATTACHMENTS})"
                    );
                    out.push(id);
                }
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut nodes = 0;
    for key in [
        "Files",
        "FileIDs",
        "FileID",
        "DocumentID",
        "File",
        "Document",
        "Documents",
        "Attachments",
    ] {
        collect(&value[key], &mut out, &mut seen, &mut nodes, 0)?;
    }
    Ok(out)
}

/// Deduplicate before counting, and stop before allocating or requesting an oversized batch.
pub fn limited_file_ids(ids: impl IntoIterator<Item = i64>) -> Result<Vec<i64>> {
    let mut seen = HashSet::new();
    for id in ids.into_iter().filter(|id| *id > 0) {
        seen.insert(id);
        ensure!(
            seen.len() <= MAX_ATTACHMENTS,
            "too many file references (maximum {MAX_ATTACHMENTS})"
        );
    }
    let mut ids: Vec<_> = seen.into_iter().collect();
    ids.sort_unstable();
    Ok(ids)
}

pub fn parse_schedule(value: &Value) -> Result<Vec<ScheduleDay>> {
    let Some(items) = value.as_array() else {
        bail!("schedule response is not an array")
    };
    if items.is_empty() {
        return Ok(Vec::new());
    }
    if items[0].get("Date").is_some() {
        let mut days = Vec::with_capacity(items.len());
        for raw in items {
            match lenient_day(raw) {
                Some(day) => days.push(day),
                None => tracing::warn!("skipping schedule day without a usable date"),
            }
        }
        ensure!(!days.is_empty(), "schedule contains no usable days");
        return Ok(days);
    }
    let wrapper = &items[0];
    let list = wrapper["DayList"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("schedule has neither Date nor DayList"))?;
    let mut days = Vec::with_capacity(list.len());
    for d in list {
        let date = d["Date"].as_str().unwrap_or_default();
        if date.is_empty() {
            tracing::warn!("skipping schedule day without a usable date");
            continue;
        }
        let mut day = ScheduleDay {
            date: date.into(),
            today: d["Today"].as_bool().unwrap_or(false),
            max_pair: wrapper["MaxPair"].as_i64().unwrap_or(0),
            ..Default::default()
        };
        if let Some(pairs) = d["Pairs"].as_array() {
            for pair in pairs {
                if let Some(groups) = pair["Subgroups"].as_object() {
                    for (group, subjects) in groups {
                        if let Some(subjects) = subjects.as_array() {
                            for subject in subjects {
                                match serde_json::from_value::<Subject>(subject.clone()) {
                                    Ok(mut subject) => {
                                        subject.pair = pair["Number"].as_i64().unwrap_or(0);
                                        subject.subgroup = group.clone();
                                        if day.call_preset == 0 {
                                            day.call_preset = subject.call_preset;
                                        }
                                        day.subjects.push(subject);
                                    }
                                    Err(error) => {
                                        tracing::warn!(
                                            %error,
                                            "skipping malformed subject"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        days.push(day);
    }
    ensure!(!days.is_empty(), "schedule contains no usable days");
    Ok(days)
}

/// Parse one `Date`-shaped day, skipping malformed lessons instead of
/// failing the whole week when the site changes a field.
fn lenient_day(raw: &Value) -> Option<ScheduleDay> {
    let mut shell = raw.clone();
    if let Some(map) = shell.as_object_mut() {
        map.insert("Subjects".into(), Value::Array(Vec::new()));
    }
    let mut day: ScheduleDay = serde_json::from_value(shell).ok()?;
    if day.date.is_empty() {
        return None;
    }
    if let Some(subjects) = raw.get("Subjects").and_then(Value::as_array) {
        for subject in subjects {
            match serde_json::from_value::<Subject>(subject.clone()) {
                Ok(subject) => day.subjects.push(subject),
                Err(error) => tracing::warn!(%error, "skipping malformed subject"),
            }
        }
    }
    Some(day)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Hall {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub number: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Call {
    #[serde(deserialize_with = "null_default")]
    pub r#break: i64,
    #[serde(deserialize_with = "null_default")]
    pub duration: i64,
    #[serde(deserialize_with = "null_default")]
    pub pair_number: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Preset {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub begin: String,
    #[serde(deserialize_with = "null_default")]
    pub call_set: Vec<Call>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PairType {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub name: String,
    #[serde(deserialize_with = "null_default")]
    pub billing_type: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Absence {
    #[serde(deserialize_with = "null_default")]
    pub caption: String,
    #[serde(deserialize_with = "null_default")]
    pub digit: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Document {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub caption: String,
    #[serde(deserialize_with = "null_default")]
    pub icon: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct ReferenceData {
    pub halls: HashMap<i64, Hall>,
    pub presets: HashMap<i64, Preset>,
    pub types: HashMap<i64, PairType>,
    pub absence: HashMap<i64, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct View {
    pub date: NaiveDate,
    pub week_start: NaiveDate,
    pub group: Option<i64>,
    pub subgroup: String,
    pub show_all: bool,
    pub week_offset: i32,
    pub screen: Screen,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum Screen {
    #[default]
    Schedule,
    Weeks,
    GroupInput,
}

impl View {
    pub fn own(a: &crate::storage::Account, date: NaiveDate) -> Self {
        Self {
            date,
            week_start: week_start(date),
            group: None,
            subgroup: a.subgroup.clone(),
            show_all: a.show_all,
            week_offset: 0,
            screen: Screen::Schedule,
        }
    }
    pub fn week(&self) -> NaiveDate {
        self.week_start
    }
    pub fn select_date(&mut self, date: NaiveDate) {
        self.date = date;
        self.week_start = week_start(date);
    }
    pub fn scope(&self, a: &crate::storage::Account) -> String {
        if let Some(group) = self.group {
            format!("group:{group}")
        } else if !a.teacher_hash.is_empty() {
            format!("teacher:{}", a.teacher_hash)
        } else if self.show_all || self.subgroup != a.personal_subgroup {
            format!("group:{}", a.group_id)
        } else {
            "personal".into()
        }
    }
}

pub fn personal_subgroup(value: &str) -> Option<&'static str> {
    match value.to_lowercase().replace([' ', '_', '-'], "").as_str() {
        "1"
        | "left"
        | "first"
        | "one"
        | "первая"
        | "первый"
        | "1я"
        | "1ая"
        | "1ый"
        | "1подгруппа"
        | "1яподгруппа"
        | "1аяподгруппа"
        | "подгруппа1" => Some("left"),
        "2"
        | "right"
        | "second"
        | "two"
        | "вторая"
        | "второй"
        | "2я"
        | "2ая"
        | "2ой"
        | "2подгруппа"
        | "2яподгруппа"
        | "2аяподгруппа"
        | "подгруппа2" => Some("right"),
        _ => None,
    }
}

pub fn subgroup_label(value: &str) -> &'static str {
    match personal_subgroup(value) {
        Some("left") => "1 подгруппа",
        Some("right") => "2 подгруппа",
        _ => "общая",
    }
}

pub fn week_start(date: NaiveDate) -> NaiveDate {
    date.checked_sub_days(Days::new(date.weekday().num_days_from_monday().into()))
        .unwrap_or(date)
}

pub fn week_millis(date: NaiveDate, tz: Tz) -> Result<i64> {
    Ok(tz
        .from_local_datetime(&week_start(date).and_hms_opt(6, 0, 0).unwrap())
        .earliest()
        .ok_or_else(|| anyhow::anyhow!("invalid local week start"))?
        .timestamp_millis())
}

pub fn day_date(raw: &str, tz: Tz) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(raw)
        .map(|d| d.with_timezone(&tz).date_naive())
        .ok()
        .or_else(|| {
            raw.get(..10)
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
        })
}

pub fn parse_date(raw: &str, today: NaiveDate) -> Result<NaiveDate> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(today);
    }
    for format in ["%Y-%m-%d", "%d.%m.%Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(raw, format) {
            return Ok(d);
        }
    }
    NaiveDate::parse_from_str(&format!("{raw}.{}", today.year()), "%d.%m.%Y").map_err(Into::into)
}

pub fn visible_subjects<'a>(day: &'a ScheduleDay, view: &View, teacher: bool) -> Vec<&'a Subject> {
    day.subjects
        .iter()
        .filter(|s| {
            teacher
                || view.show_all
                || personal_subgroup(&s.subgroup).is_none_or(|sub| sub == view.subgroup)
        })
        .collect()
}

pub fn non_school_day(day: &ScheduleDay, view: &View, teacher: bool) -> bool {
    visible_subjects(day, view, teacher)
        .iter()
        .all(|s| s.extended_data.pair_type == 9 || s.extra_data.lecture_type == 9)
}
