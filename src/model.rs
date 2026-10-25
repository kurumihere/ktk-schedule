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
    if value.is_object()
        && let Ok((days, _)) = parse_digital_schedule(value)
    {
        return Ok(days);
    }
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
    #[serde(deserialize_with = "null_default")]
    pub building: String,
}

impl Hall {
    pub fn parts(&self) -> (&str, &str) {
        if !self.building.is_empty() {
            (self.number.as_str(), self.building.as_str())
        } else if let Some((num, b)) = self.number.split_once(" (")
            && let Some(b) = b.strip_suffix(')')
            && !b.is_empty()
        {
            (num, b)
        } else {
            (self.number.as_str(), "")
        }
    }

    pub fn display(&self, show_building: bool) -> String {
        let (num, b) = self.parts();
        if show_building && !b.is_empty() {
            format!("{num} ({b})")
        } else {
            num.to_string()
        }
    }
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

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct ReferenceData {
    pub halls: HashMap<i64, Hall>,
    pub presets: HashMap<i64, Preset>,
    pub types: HashMap<i64, PairType>,
    pub absence: HashMap<i64, String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    pub cookie: String,
    pub session_id: String,
    pub group: i64,
    pub subgroup: String,
    pub teacher: String,
    pub branch: String,
    pub personal_endpoint: String,
    pub group_endpoint: String,
}

pub fn decode_seroval(raw: &Value) -> Value {
    fn inner(node: &Value, map: &mut HashMap<i64, Value>) -> Value {
        match node {
            Value::Object(obj) => {
                let id = obj.get("i").and_then(Value::as_i64);
                if let Some(i) = id
                    && let Some(cached) = map.get(&i)
                {
                    return cached.clone();
                }
                let t = obj.get("t").and_then(Value::as_i64);
                let result = match t {
                    Some(0 | 1) => obj.get("s").cloned().unwrap_or(Value::Null),
                    Some(2) => match obj.get("s").and_then(Value::as_i64) {
                        Some(2) => Value::Bool(true),
                        Some(3) => Value::Bool(false),
                        _ => Value::Null,
                    },
                    Some(3..=5) => obj.get("s").cloned().unwrap_or(Value::Null),
                    Some(9) => {
                        let arr = obj.get("a").and_then(Value::as_array);
                        match arr {
                            Some(list) => {
                                let decoded: Vec<Value> =
                                    list.iter().map(|item| inner(item, map)).collect();
                                Value::Array(decoded)
                            }
                            None => Value::Array(Vec::new()),
                        }
                    }
                    Some(10) => {
                        let p = obj.get("p");
                        let keys = p.and_then(|p| p.get("k")).and_then(Value::as_array);
                        let values = p.and_then(|p| p.get("v")).and_then(Value::as_array);
                        match (keys, values) {
                            (Some(k_list), Some(v_list)) => {
                                let mut out_map = serde_json::Map::with_capacity(k_list.len());
                                for (k, v) in k_list.iter().zip(v_list.iter()) {
                                    if let Some(key_str) = k.as_str() {
                                        out_map.insert(key_str.to_string(), inner(v, map));
                                    }
                                }
                                Value::Object(out_map)
                            }
                            _ => Value::Object(serde_json::Map::new()),
                        }
                    }
                    Some(11) => {
                        let p = obj.get("p");
                        let keys = p.and_then(|p| p.get("k")).and_then(Value::as_array);
                        match keys {
                            Some(k_list) => {
                                let decoded: Vec<Value> =
                                    k_list.iter().map(|item| inner(item, map)).collect();
                                Value::Array(decoded)
                            }
                            None => Value::Array(Vec::new()),
                        }
                    }
                    _ => {
                        let mut out_map = serde_json::Map::with_capacity(obj.len());
                        for (k, v) in obj {
                            out_map.insert(k.clone(), inner(v, map));
                        }
                        Value::Object(out_map)
                    }
                };
                if let Some(i) = id {
                    map.insert(i, result.clone());
                }
                result
            }
            Value::Array(arr) => Value::Array(arr.iter().map(|item| inner(item, map)).collect()),
            primitive => primitive.clone(),
        }
    }

    let mut map = HashMap::new();
    let decoded = inner(raw, &mut map);
    if let Some(res) = decoded.get("result") {
        res.clone()
    } else {
        decoded
    }
}

pub fn parse_digital_schedule(value: &Value) -> Result<(Vec<ScheduleDay>, ReferenceData)> {
    let root = if value.get("t").is_some() || value.get("result").is_some() {
        decode_seroval(value)
    } else {
        value.clone()
    };
    let days_raw = root
        .get("days")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("digital schedule missing days array"))?;

    let refs_obj = root.get("references");
    let mut teachers = HashMap::new();
    if let Some(t_list) = refs_obj
        .and_then(|r| r.get("teachers"))
        .and_then(Value::as_array)
    {
        for t in t_list {
            if let Some(id) = t.get("id").and_then(Value::as_str) {
                let s = t.get("surname").and_then(Value::as_str).unwrap_or_default();
                let n = t.get("name").and_then(Value::as_str).unwrap_or_default();
                let p = t
                    .get("patronymic")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let full = format!("{s} {n} {p}").trim().to_string();
                teachers.insert(id.to_string(), full);
            }
        }
    }

    let mut halls = HashMap::new();
    if let Some(a_list) = refs_obj
        .and_then(|r| r.get("auditoriums"))
        .and_then(Value::as_array)
    {
        for a in a_list {
            if let Some(id) = a.get("auditoriumId").and_then(Value::as_i64) {
                let code = a
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let b_code = a
                    .get("buildingShortCode")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                let is_virtual =
                    a.get("variant").and_then(Value::as_str) == Some("virtual") || code == "Д";
                let (number, building) = if is_virtual {
                    ("Дистанционно".to_string(), String::new())
                } else {
                    (code, b_code.to_string())
                };
                halls.insert(
                    id,
                    Hall {
                        id,
                        number,
                        building,
                    },
                );
            }
        }
    }

    let mut types = HashMap::new();
    if let Some(pt_list) = refs_obj
        .and_then(|r| r.get("pairTypes"))
        .and_then(Value::as_array)
    {
        for pt in pt_list {
            if let Some(id) = pt.get("id").and_then(Value::as_i64) {
                let name = pt
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let billing = pt
                    .get("billingType")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                types.insert(
                    id,
                    PairType {
                        id,
                        name,
                        billing_type: billing,
                    },
                );
            }
        }
    }

    let mut presets = HashMap::new();
    if let Some(cp_list) = root.get("callPresets").and_then(Value::as_array) {
        for cp in cp_list {
            if let Some(id) = cp.get("id").and_then(Value::as_i64) {
                let begin = cp
                    .get("beginTime")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut call_set = Vec::new();
                if let Some(steps) = cp.get("steps").and_then(Value::as_array) {
                    for step in steps {
                        let pos = step.get("position").and_then(Value::as_i64).unwrap_or(0);
                        let duration = step.get("duration").and_then(Value::as_i64).unwrap_or(0);
                        let break_duration = step
                            .get("breakDuration")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        call_set.push(Call {
                            duration,
                            r#break: break_duration,
                            pair_number: pos + 1,
                        });
                    }
                }
                presets.insert(
                    id,
                    Preset {
                        id,
                        begin,
                        call_set,
                    },
                );
            }
        }
    }

    let mut bell_assignments = HashMap::new();
    if let Some(scopes) = root.get("bellScopes").and_then(Value::as_array) {
        for scope in scopes {
            if let Some(assignments) = scope.get("assignments").and_then(Value::as_array) {
                for a in assignments {
                    if let (Some(date), Some(preset_id)) = (
                        a.get("date").and_then(Value::as_str),
                        a.get("presetId").and_then(Value::as_i64),
                    ) {
                        bell_assignments.insert(date.to_string(), preset_id);
                    }
                }
            }
        }
    }

    let mut schedule_days = Vec::with_capacity(days_raw.len());
    let max_pair = root
        .get("maxPairNumber")
        .and_then(Value::as_i64)
        .unwrap_or(9);
    for d in days_raw {
        let date = d.get("date").and_then(Value::as_str).unwrap_or_default();
        if date.is_empty() {
            continue;
        }
        let call_preset = bell_assignments.get(date).copied().unwrap_or(0);
        let mut day = ScheduleDay {
            call_preset,
            date: date.to_string(),
            today: false,
            max_pair,
            subjects: Vec::new(),
        };
        let parse_lesson =
            |pair_number: i64, lesson: &serde_json::Map<String, Value>| -> Option<Subject> {
                let discipline = lesson
                    .get("disciplineShortName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if discipline.is_empty() {
                    return None;
                }
                let discipline_full = lesson
                    .get("disciplineFullName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let auditorium_id = lesson
                    .get("auditoriumId")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let teacher_name = lesson
                    .get("teacherId")
                    .and_then(Value::as_str)
                    .and_then(|id| teachers.get(id))
                    .cloned()
                    .unwrap_or_default();
                let group = lesson
                    .get("groupId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let subgroup = lesson
                    .get("subgroup")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let pair_type = lesson
                    .get("pairTypeId")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let academic_hour = lesson
                    .get("academicHours")
                    .and_then(Value::as_i64)
                    .unwrap_or(2);
                let sheet = lesson.get("journalId").and_then(Value::as_i64).unwrap_or(0);
                let appraisal = lesson
                    .get("assessment")
                    .and_then(|a| a.get("grade"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let mark = lesson
                    .get("assessment")
                    .and_then(|a| a.get("mark"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);

                Some(Subject {
                    pair: pair_number,
                    discipline,
                    lecture_hall: auditorium_id,
                    teacher: teacher_name,
                    group,
                    subgroup,
                    appraisal,
                    mark,
                    call_preset,
                    extended_data: ExtendedData {
                        academic_hour,
                        discipline_full,
                        pair_type,
                    },
                    extra_data: ExtraData {
                        lecture_theme: String::new(),
                        lecture_homework: String::new(),
                        lecture_type: pair_type,
                        sheet,
                        homework: Homework::default(),
                    },
                })
            };

        if let Some(pages) = d.get("pages").and_then(Value::as_array) {
            for page in pages {
                if let Some(pairs) = page.get("pairs").and_then(Value::as_array) {
                    for p in pairs {
                        let pair_number = p.get("pairNumber").and_then(Value::as_i64).unwrap_or(0);
                        if let Some(lesson) = p.get("lesson").and_then(Value::as_object)
                            && let Some(s) = parse_lesson(pair_number, lesson)
                        {
                            day.subjects.push(s);
                        }
                    }
                }
            }
        }
        if let Some(pairs) = d.get("pairs").and_then(Value::as_array) {
            for p in pairs {
                let pair_number = p.get("pairNumber").and_then(Value::as_i64).unwrap_or(0);
                if let Some(ordinary) = p.get("ordinary").and_then(Value::as_object) {
                    for list_key in ["wholeLessons", "leftLessons", "rightLessons"] {
                        if let Some(lessons) = ordinary.get(list_key).and_then(Value::as_array) {
                            for l in lessons {
                                if let Some(lesson) = l.as_object()
                                    && let Some(mut s) = parse_lesson(pair_number, lesson)
                                {
                                    if s.subgroup.is_empty() {
                                        s.subgroup = match list_key {
                                            "leftLessons" => "left",
                                            "rightLessons" => "right",
                                            _ => "middle",
                                        }
                                        .into();
                                    }
                                    day.subjects.push(s);
                                }
                            }
                        }
                    }
                }
                if let Some(dirs) = p.get("directions").and_then(Value::as_array) {
                    for dir in dirs {
                        if let Some(lessons) = dir.get("lessons").and_then(Value::as_array) {
                            for l in lessons {
                                if let Some(lesson) = l.as_object()
                                    && let Some(s) = parse_lesson(pair_number, lesson)
                                {
                                    day.subjects.push(s);
                                }
                            }
                        }
                    }
                }
            }
        }
        day.subjects.sort_by_key(|s| s.pair);
        schedule_days.push(day);
    }
    ensure!(
        !schedule_days.is_empty(),
        "digital schedule contains no usable days"
    );
    let ref_data = ReferenceData {
        halls,
        presets,
        types,
        absence: HashMap::new(),
    };
    Ok((schedule_days, ref_data))
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
