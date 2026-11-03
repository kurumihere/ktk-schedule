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
    /// Digital platform: the lesson is in a virtual auditorium (distance learning).
    #[serde(default)]
    pub remote: bool,
    /// Digital platform: the teacher set a task; its text and files come separately.
    #[serde(default)]
    pub has_homework: bool,
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "PascalCase")]
pub struct Call {
    #[serde(deserialize_with = "null_default")]
    pub r#break: i64,
    #[serde(deserialize_with = "null_default")]
    pub duration: i64,
    #[serde(deserialize_with = "null_default")]
    pub pair_number: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "PascalCase")]
pub struct Preset {
    #[serde(rename = "ID", deserialize_with = "null_default")]
    pub id: i64,
    #[serde(deserialize_with = "null_default")]
    pub begin: String,
    #[serde(deserialize_with = "null_default")]
    pub call_set: Vec<Call>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ReferenceData {
    pub halls: HashMap<i64, Hall>,
    pub presets: HashMap<i64, Preset>,
    pub types: HashMap<i64, PairType>,
    pub absence: HashMap<i64, String>,
}

impl ReferenceData {
    pub fn is_empty(&self) -> bool {
        self.halls.is_empty()
            && self.presets.is_empty()
            && self.types.is_empty()
            && self.absence.is_empty()
    }

    /// Add or replace entries from `other`; returns whether anything changed.
    pub fn merge(&mut self, other: ReferenceData) -> bool {
        fn absorb<V: PartialEq>(into: &mut HashMap<i64, V>, from: HashMap<i64, V>) -> bool {
            let mut changed = false;
            for (id, value) in from {
                if into.get(&id) != Some(&value) {
                    into.insert(id, value);
                    changed = true;
                }
            }
            changed
        }
        // Non-short-circuiting `|`: every map must be merged.
        absorb(&mut self.halls, other.halls)
            | absorb(&mut self.presets, other.presets)
            | absorb(&mut self.types, other.types)
            | absorb(&mut self.absence, other.absence)
    }
}

/// A signed-in site session, persisted (sealed) so it survives restarts. The site extends
/// it by 30 days on every use; there is no expiry to track on our side.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Cookie header for the site: `name=value` pairs joined by `; `.
    pub cookie: String,
    pub session_id: String,
    pub group: i64,
    pub subgroup: String,
    pub teacher: String,
    pub branch: String,
    pub personal_endpoint: String,
    pub group_endpoint: String,
}

/// The cookies authenticate the account: keep them out of logs.
impl std::fmt::Debug for SessionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionRecord")
            .field("cookie", &"<redacted>")
            .field("group", &self.group)
            .field("teacher", &!self.teacher.is_empty())
            .finish_non_exhaustive()
    }
}

/// The week a request is about: its Monday, and the Monday 06:00 local timestamp the legacy
/// API expects (`week_millis`). Both come from the configured `TIMEZONE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Week {
    pub start: NaiveDate,
    pub millis: i64,
}

impl Week {
    pub fn new(date: NaiveDate, tz: Tz) -> Result<Self> {
        Ok(Self {
            start: week_start(date),
            millis: week_millis(date, tz)?,
        })
    }

    /// Academic year the week belongs to (the year of its September 1st).
    pub fn academic_year(self) -> i32 {
        academic_year_start(self.start).year()
    }
}

/// Monday of week 1 of the academic year containing `date`; week 1 contains September 1st.
/// Total for every `NaiveDate`, including the extremes chrono can represent.
pub fn academic_year_start(date: NaiveDate) -> NaiveDate {
    let start = week_start(date);
    let first_week = |year| NaiveDate::from_ymd_opt(year, 9, 1).map(week_start);
    first_week(start.year())
        .filter(|first| *first <= start)
        .or_else(|| first_week(start.year() - 1))
        .unwrap_or(start)
}

/// A student's personal timetable lists only their own subgroup's split lessons, so one
/// subgroup alone is conclusive; none or both prove nothing.
pub fn infer_subgroup(days: &[ScheduleDay]) -> Option<&'static str> {
    let mut found = None;
    for subject in days.iter().flat_map(|d| &d.subjects) {
        match (personal_subgroup(&subject.subgroup), found) {
            (Some(sub), None) => found = Some(sub),
            (Some(sub), Some(seen)) if sub != seen => return None,
            _ => (),
        }
    }
    found
}

/// References (`t: 4`) expand to copies of earlier nodes, so a few kilobytes could
/// otherwise decode into gigabytes. Real schedule weeks stay below 20 000 nodes.
const MAX_SEROVAL_NODES: usize = 1 << 20;

/// Characters seroval escapes in every string node (`serializeString`), in the order
/// `deserializeString` matches them.
const SEROVAL_ESCAPES: [(&str, char); 10] = [
    ("\\\\", '\\'),
    ("\\\"", '"'),
    ("\\n", '\n'),
    ("\\r", '\r'),
    ("\\b", '\u{8}'),
    ("\\t", '\t'),
    ("\\f", '\u{c}'),
    ("\\u2028", '\u{2028}'),
    ("\\u2029", '\u{2029}'),
    ("\\x3C", '<'),
];

/// seroval `serializeString`: strings sent to a server function must be escaped exactly as the
/// website escapes them, because the server unescapes every string node it receives.
pub fn seroval_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match SEROVAL_ESCAPES.iter().find(|(_, raw)| *raw == c) {
            Some((escaped, _)) => out.push_str(escaped),
            None => out.push(c),
        }
    }
    out
}

/// seroval `deserializeString`: left-to-right, unknown sequences stay as they are.
pub fn seroval_unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('\\') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        match SEROVAL_ESCAPES
            .iter()
            .find(|(escaped, _)| tail.starts_with(escaped))
        {
            Some((escaped, raw)) => {
                out.push(*raw);
                rest = &tail[escaped.len()..];
            }
            None => {
                out.push('\\');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A server function argument: the platform validates types, so ids go as numbers.
#[derive(Clone, Copy, Debug)]
pub enum Field<'a> {
    Text(&'a str),
    Number(i64),
}

impl<'a> From<&'a str> for Field<'a> {
    fn from(value: &'a str) -> Self {
        Self::Text(value)
    }
}

impl<'a> From<&'a String> for Field<'a> {
    fn from(value: &'a String) -> Self {
        Self::Text(value)
    }
}

/// Server function payload: `{ data: { field: value, ... } }` serialized as a seroval tree,
/// framed the way the website's own client sends it.
pub fn seroval_request(fields: &[(&str, &str)]) -> Value {
    let fields: Vec<_> = fields.iter().map(|(k, v)| (*k, Field::Text(v))).collect();
    seroval_fields(&fields)
}

/// `seroval_request` with typed values.
pub fn seroval_fields(fields: &[(&str, Field)]) -> Value {
    let (keys, values): (Vec<_>, Vec<_>) = fields
        .iter()
        .map(|(key, value)| {
            (
                seroval_escape(key),
                match value {
                    Field::Text(text) => serde_json::json!({ "t": 1, "s": seroval_escape(text) }),
                    Field::Number(number) => serde_json::json!({ "t": 0, "s": number }),
                },
            )
        })
        .unzip();
    serde_json::json!({
        "t": {
            "t": 10, "i": 0, "p": {
                "k": ["data"],
                "v": [{ "t": 10, "i": 1, "p": { "k": keys, "v": values }, "o": 0 }]
            },
            "o": 0
        },
        "f": 127,
        "m": []
    })
}

/// Decode a seroval tree (TanStack Start server function wire format) into plain JSON.
/// A request frame `{ t, f, m }` is unwrapped, and so is a `{ result }` envelope.
pub fn decode_seroval(raw: &Value) -> Result<Value> {
    let root = match raw.get("t") {
        Some(node @ Value::Object(_)) if raw.get("f").is_some() => node,
        _ => raw,
    };
    let mut decoder = Seroval::default();
    Ok(match decoder.decode(root)? {
        Value::Object(mut map) if map.contains_key("result") => {
            map.remove("result").unwrap_or_default()
        }
        decoded => decoded,
    })
}

#[derive(Default)]
struct Seroval {
    /// Node id -> decoded value and the number of nodes it expands to.
    nodes_by_id: HashMap<i64, (Value, usize)>,
    nodes: usize,
}

impl Seroval {
    // seroval `SerovalNodeType` values that can occur in JSON-compatible payloads.
    const NUMBER: i64 = 0;
    const STRING: i64 = 1;
    const CONSTANT: i64 = 2;
    const BIGINT: i64 = 3;
    const DATE: i64 = 5;
    const SET: i64 = 7;
    const MAP: i64 = 8;
    const ARRAY: i64 = 9;
    const OBJECT: i64 = 10;
    const NULL_CONSTRUCTOR: i64 = 11;

    fn charge(&mut self, nodes: usize) -> Result<()> {
        self.nodes = self.nodes.saturating_add(nodes);
        ensure!(
            self.nodes <= MAX_SEROVAL_NODES,
            "server response exceeds {MAX_SEROVAL_NODES} nodes"
        );
        Ok(())
    }

    fn decode(&mut self, node: &Value) -> Result<Value> {
        self.charge(1)?;
        let start = self.nodes;
        let map = match node {
            Value::Object(map) => map,
            Value::Array(items) => return self.list(items),
            primitive => return Ok(primitive.clone()),
        };
        let id = map.get("i").and_then(Value::as_i64);
        // Indexed values (`t: 4`) and repeated ids point at an already decoded node.
        if let Some(id) = id
            && let Some(size) = self.nodes_by_id.get(&id).map(|(_, size)| *size)
        {
            self.charge(size)?;
            return Ok(self.nodes_by_id[&id].0.clone());
        }
        let Some(kind) = map.get("t").and_then(Value::as_i64) else {
            // Plain JSON around the tree, such as an already unwrapped envelope.
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, value) in map {
                out.insert(key.clone(), self.decode(value)?);
            }
            return Ok(Value::Object(out));
        };
        let value = match kind {
            Self::STRING => Value::String(seroval_unescape(
                map.get("s").and_then(Value::as_str).unwrap_or_default(),
            )),
            Self::NUMBER | Self::BIGINT | Self::DATE => map.get("s").cloned().unwrap_or_default(),
            Self::CONSTANT => match map.get("s").and_then(Value::as_i64) {
                Some(2) => Value::Bool(true),
                Some(3) => Value::Bool(false),
                _ => Value::Null,
            },
            Self::ARRAY | Self::SET => self.list(
                map.get("a")
                    .and_then(Value::as_array)
                    .map_or(&[], Vec::as_slice),
            )?,
            Self::OBJECT | Self::NULL_CONSTRUCTOR => self.properties(map.get("p"), false)?,
            Self::MAP => self.properties(map.get("e"), true)?,
            // Promises, streams, symbols and dangling references carry no schedule data.
            _ => Value::Null,
        };
        if let Some(id) = id {
            let size = self.nodes - start + 1;
            self.nodes_by_id.insert(id, (value.clone(), size));
        }
        Ok(value)
    }

    fn list(&mut self, items: &[Value]) -> Result<Value> {
        items
            .iter()
            .map(|item| self.decode(item))
            .collect::<Result<_>>()
            .map(Value::Array)
    }

    /// Object properties carry escaped string keys; `Map` entries carry key nodes.
    fn properties(&mut self, record: Option<&Value>, key_nodes: bool) -> Result<Value> {
        let field = |name: &str| record.and_then(|r| r.get(name)).and_then(Value::as_array);
        let (Some(keys), Some(values)) = (field("k"), field("v")) else {
            return Ok(Value::Object(serde_json::Map::new()));
        };
        let mut out = serde_json::Map::with_capacity(keys.len());
        for (key, value) in keys.iter().zip(values) {
            let key = if key_nodes {
                match self.decode(key)? {
                    Value::String(key) => key,
                    Value::Number(key) => key.to_string(),
                    _ => continue,
                }
            } else if let Some(key) = key.as_str() {
                seroval_unescape(key)
            } else {
                continue;
            };
            out.insert(key, self.decode(value)?);
        }
        Ok(Value::Object(out))
    }
}

/// Parse a digital week from the raw server response (seroval) or already decoded JSON.
pub fn parse_digital_schedule(value: &Value) -> Result<(Vec<ScheduleDay>, ReferenceData)> {
    if value.get("t").is_some() || value.get("result").is_some() {
        digital_week(&decode_seroval(value)?)
    } else {
        digital_week(value)
    }
}

/// Parse a decoded digital week: lessons plus the halls, pair types and bell presets it uses.
pub fn digital_week(root: &Value) -> Result<(Vec<ScheduleDay>, ReferenceData)> {
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
    let mut remote_halls = HashSet::new();
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
                if is_virtual {
                    remote_halls.insert(id);
                }
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
                        remote: remote_halls.contains(&auditorium_id),
                        has_homework: lesson
                            .get("hasHomework")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
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
    Subgroup,
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

/// User-selected dates (commands and week buttons) stay within ten years of today:
/// nothing useful lies further away, and extreme years are not worth a Workspace request.
pub const MAX_DAYS_FROM_TODAY: i64 = 3650;

pub fn within_reach(date: NaiveDate, today: NaiveDate) -> bool {
    (date - today).num_days().abs() <= MAX_DAYS_FROM_TODAY
}

pub fn week_start(date: NaiveDate) -> NaiveDate {
    date.checked_sub_days(Days::new(date.weekday().num_days_from_monday().into()))
        .unwrap_or(date)
}

pub fn shift(date: NaiveDate, delta: i64) -> Result<NaiveDate> {
    let days = Days::new(delta.unsigned_abs());
    if delta >= 0 {
        date.checked_add_days(days)
    } else {
        date.checked_sub_days(days)
    }
    .ok_or_else(|| anyhow::anyhow!("date out of range"))
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
    let date = match ["%Y-%m-%d", "%d.%m.%Y"]
        .into_iter()
        .find_map(|format| NaiveDate::parse_from_str(raw, format).ok())
    {
        Some(date) => date,
        None => NaiveDate::parse_from_str(&format!("{raw}.{}", today.year()), "%d.%m.%Y")?,
    };
    // `%Y` also accepts "26" or "-262144"; such dates are typos and must not reach rendering.
    ensure!(within_reach(date, today), "date is too far from today");
    Ok(date)
}

/// A day by name: "сегодня", "завтра", "послезавтра", or a weekday ("чт", "пятница", "пон.")
/// as its next occurrence from `today` on, today included.
pub fn parse_day(raw: &str, today: NaiveDate) -> Option<NaiveDate> {
    const WEEKDAYS: [(&str, &str); 7] = [
        ("пн", "понедельник"),
        ("вт", "вторник"),
        ("ср", "среда"),
        ("чт", "четверг"),
        ("пт", "пятница"),
        ("сб", "суббота"),
        ("вс", "воскресенье"),
    ];
    let word = raw.trim().trim_end_matches('.').to_lowercase();
    let ahead = match word.as_str() {
        "сегодня" => 0,
        "завтра" => 1,
        "послезавтра" => 2,
        _ => {
            let weekday = WEEKDAYS.iter().position(|(short, full)| {
                word == *short || (word.chars().count() >= 3 && full.starts_with(&word))
            })?;
            (weekday as u32 + 7 - today.weekday().num_days_from_monday()) % 7
        }
    };
    today.checked_add_days(Days::new(ahead.into()))
}

pub fn visible_subjects_scoped<'a>(
    day: &'a ScheduleDay,
    view: &View,
    teacher: bool,
    is_personal: bool,
) -> Vec<&'a Subject> {
    let subjects: Vec<&Subject> = day
        .subjects
        .iter()
        .filter(|s| {
            teacher
                || is_personal
                || view.show_all
                || personal_subgroup(&s.subgroup).is_none_or(|sub| sub == view.subgroup)
        })
        .collect();
    // A student's day of distance pairs only is shown when any pair has a webinar or a task;
    // otherwise there is nothing to attend and the day is empty.
    if !teacher
        && subjects.iter().all(|s| s.extended_data.remote)
        && !subjects.iter().any(|s| has_webinar_or_task(s))
    {
        return Vec::new();
    }
    subjects
}

fn has_webinar_or_task(subject: &Subject) -> bool {
    let homework = &subject.extra_data.homework;
    let filled = |text: &Option<String>| text.as_deref().is_some_and(|t| !t.trim().is_empty());
    subject.extended_data.has_homework
        || filled(&homework.webinar)
        || filled(&homework.task)
        || !homework.files.is_empty()
}

pub fn visible_subjects<'a>(day: &'a ScheduleDay, view: &View, teacher: bool) -> Vec<&'a Subject> {
    visible_subjects_scoped(day, view, teacher, false)
}

pub fn non_school_day_scoped(
    day: &ScheduleDay,
    view: &View,
    teacher: bool,
    is_personal: bool,
) -> bool {
    visible_subjects_scoped(day, view, teacher, is_personal)
        .iter()
        .all(|s| s.extended_data.pair_type == 9 || s.extra_data.lecture_type == 9)
}

pub fn non_school_day(day: &ScheduleDay, view: &View, teacher: bool) -> bool {
    non_school_day_scoped(day, view, teacher, false)
}

pub const VK_ID_OFFSET: i64 = 1_000_000_000_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PlatformUser {
    Telegram(i64),
    Vk(i64),
}

impl PlatformUser {
    pub fn to_storage_id(self) -> i64 {
        match self {
            Self::Telegram(id) => id,
            Self::Vk(id) => VK_ID_OFFSET + id,
        }
    }

    pub fn from_storage_id(id: i64) -> Self {
        if id >= VK_ID_OFFSET {
            Self::Vk(id - VK_ID_OFFSET)
        } else {
            Self::Telegram(id)
        }
    }

    pub fn is_vk(self) -> bool {
        matches!(self, Self::Vk(_))
    }

    pub fn is_telegram(self) -> bool {
        matches!(self, Self::Telegram(_))
    }
}
