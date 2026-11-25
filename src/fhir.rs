//! The portal's records as FHIR R4 resources (records-stream/v1, method
//! `portal`).
//!
//! The portal is not a FHIR server, so every resource here is mapped, and
//! says so: `meta.source` is `fertemr:portal` and `meta.tag` carries
//! [`TAG_MAPPED`]. Ids are the portal's own record numbers with a prefix per
//! kind (`visit-`, `lab-`, `msg-`, ...), so a resource keeps its id across
//! runs and the store's same-resource merge works. Nothing claims a profile
//! (`meta.profile`): the portal gives no codes a profile would need.
//!
//! A result value the portal gives as a number keeps its spelling (`1.50`
//! stays `1.50`): see [`Doc`].

use std::collections::BTreeMap;

use base64::Engine;
use pk_cli_core::CliError;
use pk_med_records::RawResource;
use serde_json::{json, Map, Value};

use crate::model::{
    Attachment, EmbryoRecord, FileItem, FormAnswer, FormOption, FormQuestion, HealthProfile, LabResult, Message, Visit,
};

pub const TAG_SYSTEM: &str = "https://github.com/piekstra/fertemr-cli/fhir/tag";
pub const TAG_MAPPED: &str = "mapped-from-fert-portal";
pub const META_SOURCE: &str = "fertemr:portal";
/// The extension holding a status in the portal's own words, beside the
/// FHIR code it was read as.
pub const EXT_PORTAL_STATUS: &str = "https://github.com/piekstra/fertemr-cli/fhir/StructureDefinition/portal-status";
const INTERPRETATION_SYSTEM: &str = "http://terminology.hl7.org/CodeSystem/v3-ObservationInterpretation";
const CATEGORY_SYSTEM: &str = "http://terminology.hl7.org/CodeSystem/observation-category";
const DIAG_SERVICE_SYSTEM: &str = "http://terminology.hl7.org/CodeSystem/v2-0074";
const ACT_CODE_SYSTEM: &str = "http://terminology.hl7.org/CodeSystem/v3-ActCode";

/// What every mapped resource of one pull shares.
pub struct Ctx {
    /// The portal's web address, the base of its local code systems.
    pub origin: String,
    /// `Patient/<id>`.
    pub patient: String,
    /// The clinic's name, as its portal configuration gives it.
    pub clinic: String,
}

fn meta() -> Value {
    json!({
        "source": META_SOURCE,
        "tag": [{ "system": TAG_SYSTEM, "code": TAG_MAPPED, "display": "Mapped by fertemr from a FERT patient portal" }],
    })
}

/// A resource in the making. A decimal goes in as a token ([`Doc::decimal`])
/// that [`Doc::finish`] swaps for the number as the portal spelled it, since
/// `serde_json` would respell `1.50` as `1.5`.
pub struct Doc {
    token: String,
}

impl Default for Doc {
    fn default() -> Self {
        Doc::new()
    }
}

impl Doc {
    pub fn new() -> Doc {
        Doc {
            token: format!("\u{1}{}:", uuid::Uuid::new_v4().simple()),
        }
    }

    /// The JSON number `text`, when it is one FHIR's decimal allows.
    pub fn decimal(&self, text: &str) -> Option<Value> {
        is_decimal(text).then(|| Value::String(format!("{}{text}\u{1}", self.token)))
    }

    /// The resource's JSON text, its tokens replaced by their numbers.
    pub fn finish(&self, value: &Value) -> Result<RawResource, CliError> {
        let text = serde_json::to_string(value).map_err(|e| CliError::Other(format!("writing a resource: {e}")))?;
        let open = format!("\"{}", escaped(&self.token));
        let close = "\\u0001\"";
        let mut out = String::with_capacity(text.len());
        let mut rest = text.as_str();
        while let Some(start) = rest.find(&open) {
            out.push_str(&rest[..start]);
            let after = &rest[start + open.len()..];
            let end = after
                .find(close)
                .ok_or_else(|| CliError::Other("a decimal token was cut short".into()))?;
            out.push_str(&after[..end]);
            rest = &after[end + close.len()..];
        }
        out.push_str(rest);
        RawResource::parse(&out).map_err(|e| CliError::Other(format!("a mapped resource does not read back: {e}")))
    }
}

/// `serde_json`'s spelling of the token's control character.
fn escaped(token: &str) -> String {
    token.replace('\u{1}', "\\u0001")
}

/// FHIR's decimal: `-?(0|[1-9][0-9]*)(\.[0-9]+)?`, at most 30 characters.
fn is_decimal(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match body.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (body, None),
    };
    s.len() <= 30
        && !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && (int == "0" || !int.starts_with('0'))
        && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}

// ------------------------------------------------------------------ dates

/// A FHIR `dateTime` from the portal's date text: kept with its time when
/// it carries an offset (ISO 8601), the day alone when it does not (a time
/// without a zone is not a FHIR dateTime), and `M/D/YYYY` as the day. The
/// .NET and portal placeholders (years before 1902) are no date.
pub fn datetime(s: &str) -> Option<String> {
    let s = s.trim();
    if let Some(day) = us_day(s) {
        return Some(day);
    }
    let day = s.get(..10)?;
    if !is_day(day) || day < "1902-01-01" {
        return None;
    }
    let Some(rest) = s.get(10..).filter(|r| !r.is_empty()) else {
        return Some(day.to_string());
    };
    let time = rest.strip_prefix('T').or_else(|| rest.strip_prefix(' '))?;
    let (clock, zone) = split_zone(time);
    let Some(zone) = zone else {
        return Some(day.to_string());
    };
    let (hms, frac) = match clock.split_once('.') {
        Some((h, f)) => (h, Some(f)),
        None => (clock, None),
    };
    let hms = match hms.len() {
        5 => format!("{hms}:00"),
        8 => hms.to_string(),
        _ => return Some(day.to_string()),
    };
    let ok = hms.bytes().enumerate().all(|(i, b)| {
        if i == 2 || i == 5 {
            b == b':'
        } else {
            b.is_ascii_digit()
        }
    });
    if !ok || &hms[..2] > "23" || &hms[3..5] > "59" || &hms[6..8] > "60" {
        return Some(day.to_string());
    }
    let frac = frac
        .filter(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
        .map(|f| format!(".{}", &f[..f.len().min(9)]))
        .unwrap_or_default();
    Some(format!("{day}T{hms}{frac}{zone}"))
}

/// A FHIR `instant`: a [`datetime`] with its time and zone, else none.
pub fn instant(s: &str) -> Option<String> {
    datetime(s).filter(|d| d.len() > 10)
}

/// The day (`YYYY-MM-DD`) a date text names.
pub fn day(s: &str) -> Option<String> {
    datetime(s).map(|d| d[..10].to_string())
}

fn is_day(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
        && ("01"..="12").contains(&&s[5..7])
        && ("01"..="31").contains(&&s[8..10])
}

/// `M/D/YYYY`, with anything after a space ignored.
fn us_day(s: &str) -> Option<String> {
    let date = s.split_whitespace().next()?;
    let mut parts = date.split('/');
    let (m, d, y) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || y.len() != 4 || m.is_empty() || m.len() > 2 || d.is_empty() || d.len() > 2 {
        return None;
    }
    let all_digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if !(all_digits(m) && all_digits(d) && all_digits(y)) {
        return None;
    }
    let day = format!("{y}-{m:0>2}-{d:0>2}");
    (is_day(&day) && day.as_str() >= "1902-01-01").then_some(day)
}

/// A clock time and its zone (`Z`, `+hh:mm`, `-hh:mm`, `+hhmm`), the zone
/// normalized to `+hh:mm`.
fn split_zone(time: &str) -> (&str, Option<String>) {
    if let Some(clock) = time.strip_suffix('Z') {
        return (clock, Some("Z".into()));
    }
    let Some(pos) = time.rfind(['+', '-']) else {
        return (time, None);
    };
    let (clock, zone) = time.split_at(pos);
    let digits: String = zone[1..].chars().filter(|c| *c != ':').collect();
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) || &digits[..2] > "14" {
        return (time, None);
    }
    (clock, Some(format!("{}{}:{}", &zone[..1], &digits[..2], &digits[2..])))
}

// --------------------------------------------------------------- resources

fn set(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(v) = value {
        map.insert(key.to_string(), v);
    }
}

fn text_of(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn portal_status(status: &str) -> Value {
    json!([{ "url": EXT_PORTAL_STATUS, "valueString": status }])
}

/// The patient, with the name the sign-in answered.
pub fn patient(ctx: &Ctx, id: &str, full: Option<&str>, first: Option<&str>, last: Option<&str>) -> Value {
    let mut p = Map::new();
    p.insert("resourceType".into(), json!("Patient"));
    p.insert("id".into(), json!(id));
    p.insert("meta".into(), meta());
    p.insert(
        "identifier".into(),
        json!([{ "system": format!("{}/patient-id", ctx.origin), "value": id }]),
    );
    let mut name = Map::new();
    set(&mut name, "text", full.map(|s| json!(s)));
    set(&mut name, "family", last.map(|s| json!(s)));
    set(&mut name, "given", first.map(|s| json!([s])));
    if !name.is_empty() {
        p.insert("name".into(), json!([Value::Object(name)]));
    }
    Value::Object(p)
}

/// An appointment as an Encounter: `Status` read as FHIR's status, and kept
/// in the portal's words in [`EXT_PORTAL_STATUS`].
pub fn encounter(ctx: &Ctx, v: &Visit, today: &str) -> Option<Value> {
    let id = v.id?;
    let start = v.visit_date.as_deref().and_then(datetime);
    let status_text = text_of(&v.status);
    let status = encounter_status(status_text.as_deref(), start.as_deref(), today);
    let mut e = Map::new();
    e.insert("resourceType".into(), json!("Encounter"));
    e.insert("id".into(), json!(format!("visit-{id}")));
    e.insert("meta".into(), meta());
    set(&mut e, "extension", status_text.as_deref().map(portal_status));
    e.insert("status".into(), json!(status));
    e.insert(
        "class".into(),
        json!({ "system": ACT_CODE_SYSTEM, "code": "AMB", "display": "ambulatory" }),
    );
    set(&mut e, "type", text_of(&v.kind).map(|t| json!([{ "text": t }])));
    e.insert("subject".into(), json!({ "reference": ctx.patient }));
    set(&mut e, "period", start.map(|s| json!({ "start": s })));
    let minutes = v.appt_duration.as_deref().filter(|d| is_decimal(d)).and_then(|d| {
        let unit = v
            .appt_duration_units
            .as_deref()
            .unwrap_or("minutes")
            .to_ascii_lowercase();
        let (code, name) = if unit.starts_with("hour") {
            ("h", "hours")
        } else if unit.starts_with("min") {
            ("min", "minutes")
        } else {
            return None;
        };
        d.parse::<u64>()
            .ok()
            .map(|n| json!({ "value": n, "unit": name, "system": "http://unitsofmeasure.org", "code": code }))
    });
    set(&mut e, "length", minutes);
    if let Some(facility) = text_of(&v.facility) {
        let mut address = Vec::new();
        for part in [&v.addr1, &v.addr2] {
            if let Some(p) = text_of(part) {
                address.push(p);
            }
        }
        let place: Vec<String> = [&v.city, &v.state, &v.zip].iter().filter_map(|p| text_of(p)).collect();
        if !place.is_empty() {
            address.push(place.join(" "));
        }
        let display = if address.is_empty() {
            facility
        } else {
            format!("{facility}, {}", address.join(", "))
        };
        e.insert("location".into(), json!([{ "location": { "display": display } }]));
    }
    e.insert("serviceProvider".into(), json!({ "display": ctx.clinic }));
    Some(Value::Object(e))
}

/// FHIR's Encounter.status for the portal's words; `planned` or `unknown`
/// by the date when the words say nothing FHIR has.
fn encounter_status(status: Option<&str>, start: Option<&str>, today: &str) -> &'static str {
    let s = status.unwrap_or_default().to_ascii_lowercase();
    if s.contains("cancel") || s.contains("no show") || s.contains("noshow") || s.contains("no-show") {
        "cancelled"
    } else if s.contains("error") {
        "entered-in-error"
    } else if s.contains("out") || s.contains("complete") || s.contains("seen") || s.contains("finish") {
        "finished"
    } else if s.contains("in progress") || s.contains("with provider") || s.contains("roomed") {
        "in-progress"
    } else if s.contains("arriv") || s.contains("checked in") || s.contains("check in") {
        "arrived"
    } else if s.contains("sched") || s.contains("confirm") || s.contains("book") || s.contains("pending") {
        "planned"
    } else {
        match start.map(|d| &d[..10]) {
            Some(d) if d > today => "planned",
            _ => "unknown",
        }
    }
}

/// A result's id: its result number, else its order and test.
pub fn lab_id(r: &LabResult) -> Option<String> {
    match (r.lab_result_id, r.order_id, r.lab_test_id) {
        (Some(id), _, _) if id > 0 => Some(format!("lab-{id}")),
        (_, Some(o), Some(t)) => Some(format!("lab-{o}-{t}")),
        _ => None,
    }
}

fn lab_status(s: Option<&str>) -> &'static str {
    let s = s.unwrap_or_default().to_ascii_lowercase();
    if s.contains("cancel") {
        "cancelled"
    } else if s.contains("correct") || s.contains("amend") || s.contains("revis") {
        "amended"
    } else if s.contains("prelim") || s.contains("partial") || s.contains("pending") || s.contains("progress") {
        "preliminary"
    } else if s.contains("final") || s.contains("complete") || s.contains("resulted") || s.contains("reviewed") {
        "final"
    } else {
        "unknown"
    }
}

/// An abnormal flag as FHIR's interpretation.
fn interpretation(flag: &str) -> Value {
    let code = match flag.trim().to_ascii_uppercase().as_str() {
        "H" | "HIGH" => Some(("H", "High")),
        "L" | "LOW" => Some(("L", "Low")),
        "HH" | "CRITICAL HIGH" => Some(("HH", "Critical high")),
        "LL" | "CRITICAL LOW" => Some(("LL", "Critical low")),
        "A" | "ABN" | "ABNORMAL" | "Y" | "YES" => Some(("A", "Abnormal")),
        "AA" => Some(("AA", "Critical abnormal")),
        "N" | "NORMAL" | "NO" => Some(("N", "Normal")),
        _ => None,
    };
    match code {
        Some((c, d)) => {
            json!([{ "coding": [{ "system": INTERPRETATION_SYSTEM, "code": c, "display": d }], "text": flag.trim() }])
        }
        None => json!([{ "text": flag.trim() }]),
    }
}

/// A value: a quantity when it is a number (with a leading `<`, `<=`, `>`
/// or `>=` as its comparator), else the text.
fn value(doc: &Doc, text: &str, unit: Option<&str>) -> (String, Value) {
    let t = text.trim();
    let (comparator, number) = ["<=", ">=", "<", ">"]
        .iter()
        .find_map(|c| t.strip_prefix(c).map(|rest| (Some(*c), rest.trim())))
        .unwrap_or((None, t));
    if let Some(n) = doc.decimal(number) {
        let mut q = Map::new();
        q.insert("value".into(), n);
        set(&mut q, "comparator", comparator.map(|c| json!(c)));
        set(
            &mut q,
            "unit",
            unit.map(str::trim).filter(|u| !u.is_empty()).map(|u| json!(u)),
        );
        return ("valueQuantity".into(), Value::Object(q));
    }
    ("valueString".into(), json!(t))
}

/// One result line as an Observation.
pub fn lab_observation(ctx: &Ctx, doc: &Doc, r: &LabResult) -> Option<Value> {
    let id = lab_id(r)?;
    let mut o = Map::new();
    o.insert("resourceType".into(), json!("Observation"));
    o.insert("id".into(), json!(id));
    o.insert("meta".into(), meta());
    let status_text = text_of(&r.lab_status_str);
    set(&mut o, "extension", status_text.as_deref().map(portal_status));
    set(
        &mut o,
        "identifier",
        text_of(&r.accession_id).map(|a| json!([{ "system": format!("{}/lab-accession", ctx.origin), "value": a }])),
    );
    o.insert("status".into(), json!(lab_status(status_text.as_deref())));
    o.insert(
        "category".into(),
        json!([{ "coding": [{ "system": CATEGORY_SYSTEM, "code": "laboratory", "display": "Laboratory" }] }]),
    );
    let name = text_of(&r.test_name).or_else(|| text_of(&r.test_description));
    let mut code = Map::new();
    if let Some(c) = text_of(&r.test_code) {
        let mut coding = Map::new();
        coding.insert("system".into(), json!(format!("{}/lab-test", ctx.origin)));
        coding.insert("code".into(), json!(c));
        set(&mut coding, "display", name.clone().map(|n| json!(n)));
        code.insert("coding".into(), json!([Value::Object(coding)]));
    }
    code.insert("text".into(), json!(name.unwrap_or_else(|| "Lab result".into())));
    o.insert("code".into(), Value::Object(code));
    o.insert("subject".into(), json!({ "reference": ctx.patient }));
    let effective = [&r.specimen_collection_date, &r.test_request_date]
        .iter()
        .find_map(|d| d.as_deref().and_then(datetime));
    set(&mut o, "effectiveDateTime", effective.map(|d| json!(d)));
    let issued = [&r.report_date, &r.assay_completion_date]
        .iter()
        .find_map(|d| d.as_deref().and_then(instant));
    set(&mut o, "issued", issued.map(|d| json!(d)));
    set(
        &mut o,
        "performer",
        text_of(&r.lab_location_str).map(|l| json!([{ "display": l }])),
    );
    let raw = text_of(&r.result_value).or_else(|| text_of(&r.value_out));
    let unit = text_of(&r.result_unit).or_else(|| text_of(&r.value_unit));
    match raw {
        Some(v) => {
            let (key, val) = value(doc, &v, unit.as_deref());
            o.insert(key, val);
        }
        None => {
            o.insert(
                "dataAbsentReason".into(),
                json!({ "coding": [{ "system": "http://terminology.hl7.org/CodeSystem/data-absent-reason", "code": "unknown" }], "text": "No value in the portal" }),
            );
        }
    }
    let flag = text_of(&r.result_abnormal).or_else(|| text_of(&r.value_out_flag));
    set(&mut o, "interpretation", flag.as_deref().map(interpretation));
    let notes: Vec<Value> = [&r.result_note, &r.lab_user_note, &r.observation_text]
        .iter()
        .filter_map(|n| text_of(n))
        .map(|n| json!({ "text": n }))
        .collect();
    if !notes.is_empty() {
        o.insert("note".into(), Value::Array(notes));
    }
    let range = text_of(&r.result_ref_range).or_else(|| text_of(&r.ref_range));
    set(&mut o, "referenceRange", range.map(|t| json!([{ "text": t }])));
    Some(Value::Object(o))
}

/// An order's results as a DiagnosticReport, with its report PDF when the
/// portal has one (`Binary/<pdf>`, written right after it).
pub fn lab_report(ctx: &Ctx, order: i64, lines: &[&LabResult], pdf: Option<(&str, Option<&str>)>) -> Value {
    let first = lines.first();
    let mut d = Map::new();
    d.insert("resourceType".into(), json!("DiagnosticReport"));
    d.insert("id".into(), json!(format!("lab-order-{order}")));
    d.insert("meta".into(), meta());
    d.insert(
        "identifier".into(),
        json!([{ "system": format!("{}/lab-order", ctx.origin), "value": order.to_string() }]),
    );
    let status = first.map_or("unknown", |r| lab_status(r.lab_status_str.as_deref()));
    d.insert(
        "status".into(),
        json!(if status == "preliminary" { "preliminary" } else { status }),
    );
    d.insert(
        "category".into(),
        json!([{ "coding": [{ "system": DIAG_SERVICE_SYSTEM, "code": "LAB", "display": "Laboratory" }] }]),
    );
    let names: Vec<String> = {
        let mut seen = Vec::new();
        for r in lines {
            if let Some(n) = text_of(&r.test_name).or_else(|| text_of(&r.category)) {
                if !seen.contains(&n) {
                    seen.push(n);
                }
            }
        }
        seen
    };
    let title = match names.len() {
        0 => "Lab order".to_string(),
        1 => names[0].clone(),
        _ => text_of(&first.and_then(|r| r.category.clone())).unwrap_or_else(|| names.join(", ")),
    };
    d.insert("code".into(), json!({ "text": title }));
    d.insert("subject".into(), json!({ "reference": ctx.patient }));
    let effective = lines.iter().find_map(|r| {
        r.specimen_collection_date
            .as_deref()
            .or(r.test_request_date.as_deref())
            .and_then(datetime)
    });
    set(&mut d, "effectiveDateTime", effective.map(|e| json!(e)));
    let issued = lines.iter().find_map(|r| r.report_date.as_deref().and_then(instant));
    set(&mut d, "issued", issued.map(|i| json!(i)));
    set(
        &mut d,
        "performer",
        first
            .and_then(|r| text_of(&r.lab_location_str))
            .map(|l| json!([{ "display": l }])),
    );
    let results: Vec<Value> = lines
        .iter()
        .filter_map(|r| lab_id(r))
        .map(|id| json!({ "reference": format!("Observation/{id}") }))
        .collect();
    if !results.is_empty() {
        d.insert("result".into(), Value::Array(results));
    }
    if let Some((binary, title)) = pdf {
        let mut a = Map::new();
        a.insert("contentType".into(), json!("application/pdf"));
        a.insert("url".into(), json!(format!("Binary/{binary}")));
        set(&mut a, "title", title.map(|t| json!(t)));
        d.insert("presentedForm".into(), json!([Value::Object(a)]));
    }
    Value::Object(d)
}

/// One embryology record as an Observation, its columns as components.
pub fn embryo_observation(ctx: &Ctx, e: &EmbryoRecord, published: Option<&str>) -> Option<Value> {
    let id = e.record_id?;
    let mut o = Map::new();
    o.insert("resourceType".into(), json!("Observation"));
    o.insert("id".into(), json!(format!("embryo-{id}")));
    o.insert("meta".into(), meta());
    let status_text = text_of(&e.record_status);
    set(&mut o, "extension", status_text.as_deref().map(portal_status));
    o.insert("status".into(), json!("final"));
    o.insert(
        "category".into(),
        json!([{ "coding": [{ "system": CATEGORY_SYSTEM, "code": "laboratory", "display": "Laboratory" }] }]),
    );
    let label = match text_of(&e.embryo_num) {
        Some(n) => format!("Embryology record: embryo {n}"),
        None => "Embryology record".to_string(),
    };
    o.insert("code".into(), json!({ "text": label }));
    o.insert("subject".into(), json!({ "reference": ctx.patient }));
    let effective = [&e.freeze_date, &e.xfer_date, &e.discard_date]
        .iter()
        .find_map(|d| d.as_deref().and_then(datetime))
        .or_else(|| published.and_then(datetime));
    set(&mut o, "effectiveDateTime", effective.map(|d| json!(d)));
    let columns: [(&str, &Option<String>); 15] = [
        ("Embryo number", &e.embryo_num),
        ("Day", &e.record_day),
        ("Stage", &e.stage),
        ("Grade", &e.grade),
        ("PGT-A", &e.pgta),
        ("PGT, other", &e.pgt_other),
        ("Sex", &e.sex),
        ("Status", &e.record_status),
        ("Oocyte source", &e.ooc_source),
        ("Sperm source", &e.sperm_source),
        ("Device number", &e.device_num),
        ("Freeze date", &e.freeze_date),
        ("Transfer date", &e.xfer_date),
        ("Discard date", &e.discard_date),
        ("Interpretation", &e.interpretation),
    ];
    let components: Vec<Value> = columns
        .iter()
        .filter_map(|(name, v)| text_of(v).map(|v| json!({ "code": { "text": name }, "valueString": v })))
        .collect();
    if !components.is_empty() {
        o.insert("component".into(), Value::Array(components));
    }
    set(
        &mut o,
        "interpretation",
        text_of(&e.interpretation).map(|i| json!([{ "text": i }])),
    );
    set(&mut o, "note", text_of(&e.comment).map(|c| json!([{ "text": c }])));
    Some(Value::Object(o))
}

/// A file's content type: from its name, else from its first bytes.
pub fn content_type(name: Option<&str>, bytes: Option<&[u8]>) -> &'static str {
    let ext = name
        .and_then(|n| n.rsplit_once('.'))
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "heic" => "image/heic",
        "txt" => "text/plain",
        "htm" | "html" => "text/html",
        "rtf" => "application/rtf",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => match bytes.and_then(pk_cli_documents::sniff) {
            Some("pdf") => "application/pdf",
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("tiff") => "image/tiff",
            _ => "application/octet-stream",
        },
    }
}

/// A file the clinic shared, as a DocumentReference naming its Binary
/// (`binary`), or for an external file its link.
pub fn file_document(ctx: &Ctx, f: &FileItem, binary: Option<(&str, &str)>) -> Option<Value> {
    let id = f.id?;
    let name = text_of(&f.file_name);
    let mut d = Map::new();
    d.insert("resourceType".into(), json!("DocumentReference"));
    d.insert("id".into(), json!(format!("file-{id}")));
    d.insert("meta".into(), meta());
    d.insert("status".into(), json!("current"));
    set(&mut d, "type", text_of(&f.category).map(|c| json!({ "text": c })));
    set(
        &mut d,
        "category",
        text_of(&f.sub_category).map(|c| json!([{ "text": c }])),
    );
    d.insert("subject".into(), json!({ "reference": ctx.patient }));
    d.insert("custodian".into(), json!({ "display": ctx.clinic }));
    set(&mut d, "description", name.clone().map(|n| json!(n)));
    let mut a = Map::new();
    match binary {
        Some((binary, ctype)) => {
            a.insert("contentType".into(), json!(ctype));
            a.insert("url".into(), json!(format!("Binary/{binary}")));
        }
        None => {
            set(
                &mut a,
                "url",
                text_of(&f.download_link)
                    .filter(|l| l.starts_with("https://"))
                    .map(|l| json!(l)),
            );
        }
    }
    set(&mut a, "title", name.map(|n| json!(n)));
    d.insert("content".into(), json!([{ "attachment": Value::Object(a) }]));
    Some(Value::Object(d))
}

/// A message's attachment as a DocumentReference naming its Binary.
pub fn attachment_document(ctx: &Ctx, a: &Attachment, msg: &str, sent: Option<&str>, ctype: &str) -> Option<Value> {
    let id = a.id?;
    let mut d = Map::new();
    d.insert("resourceType".into(), json!("DocumentReference"));
    d.insert("id".into(), json!(format!("att-{id}")));
    d.insert("meta".into(), meta());
    d.insert("status".into(), json!("current"));
    d.insert("type".into(), json!({ "text": "Message attachment" }));
    d.insert("subject".into(), json!({ "reference": ctx.patient }));
    set(&mut d, "date", sent.and_then(instant).map(|s| json!(s)));
    d.insert("custodian".into(), json!({ "display": ctx.clinic }));
    let name = text_of(&a.file_name);
    set(&mut d, "description", name.clone().map(|n| json!(n)));
    let mut att = Map::new();
    att.insert("contentType".into(), json!(ctype));
    att.insert("url".into(), json!(format!("Binary/att-{id}")));
    set(&mut att, "title", name.map(|n| json!(n)));
    d.insert("content".into(), json!([{ "attachment": Value::Object(att) }]));
    d.insert(
        "context".into(),
        json!({ "related": [{ "reference": format!("Communication/{msg}") }] }),
    );
    Some(Value::Object(d))
}

/// The text of a message body: the portal's plain text, else the HTML with
/// its tags dropped.
fn body_text(m: &Message) -> Option<String> {
    if let Some(t) = text_of(&m.plain_text_body) {
        return Some(t);
    }
    let body = text_of(&m.body)?;
    if m.body_is_html != Some(true) && !body.contains('<') {
        return Some(body);
    }
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag = String::new();
    for c in body.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                let t = tag.trim_start_matches('/').to_ascii_lowercase();
                if t.starts_with("br") || t.starts_with("p") || t.starts_with("div") || t.starts_with("li") {
                    out.push('\n');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    let lines: Vec<&str> = out.lines().map(str::trim_end).collect();
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// A secure message as a Communication, with its attachments named by
/// their DocumentReferences and a reply naming the message it answers.
pub fn communication(ctx: &Ctx, m: &Message, in_response_to: Option<i64>, attachments: &[String]) -> Option<Value> {
    let id = m.msg_id?;
    let mut c = Map::new();
    c.insert("resourceType".into(), json!("Communication"));
    c.insert("id".into(), json!(format!("msg-{id}")));
    c.insert("meta".into(), meta());
    set(
        &mut c,
        "inResponseTo",
        in_response_to.map(|r| json!([{ "reference": format!("Communication/msg-{r}") }])),
    );
    c.insert("status".into(), json!("completed"));
    c.insert("subject".into(), json!({ "reference": ctx.patient }));
    set(&mut c, "topic", text_of(&m.subject).map(|s| json!({ "text": s })));
    set(
        &mut c,
        "sent",
        m.dt_sent_or_received.as_deref().and_then(datetime).map(|s| json!(s)),
    );
    let party = |user: &Option<String>, group: &Option<String>, patient: &Option<String>| {
        text_of(user).or_else(|| text_of(group)).or_else(|| text_of(patient))
    };
    let from = party(&m.from_user_display, &m.from_group_display, &m.from_patient_display);
    let to = party(&m.to_user_display, &m.to_group_display, &m.to_patient_display);
    let from_patient = text_of(&m.from_patient_display).is_some()
        && text_of(&m.from_user_display).is_none()
        && text_of(&m.from_group_display).is_none();
    set(
        &mut c,
        "sender",
        from.map(|f| {
            if from_patient {
                json!({ "reference": ctx.patient, "display": f })
            } else {
                json!({ "display": f })
            }
        }),
    );
    set(&mut c, "recipient", to.map(|t| json!([{ "display": t }])));
    let mut payload = Vec::new();
    if let Some(text) = body_text(m) {
        payload.push(json!({ "contentString": text }));
    }
    for a in attachments {
        payload.push(json!({ "contentReference": { "reference": format!("DocumentReference/{a}") } }));
    }
    if !payload.is_empty() {
        c.insert("payload".into(), Value::Array(payload));
    }
    Some(Value::Object(c))
}

/// A form's answers as a QuestionnaireResponse: a group per section, an
/// item per answered question in the form's order. An answer that names
/// one of the question's options is the option's text.
pub fn questionnaire_response(ctx: &Ctx, hp: &HealthProfile) -> Option<Value> {
    let inst = hp.instance.as_ref()?;
    let id = inst.id?;
    let mut q = Map::new();
    q.insert("resourceType".into(), json!("QuestionnaireResponse"));
    q.insert("id".into(), json!(format!("form-{id}")));
    q.insert("meta".into(), meta());
    q.insert(
        "identifier".into(),
        json!({ "system": format!("{}/health-profile", ctx.origin), "value": id.to_string() }),
    );
    let submitted = inst.submit_dt.as_deref().and_then(datetime);
    q.insert(
        "status".into(),
        json!(if submitted.is_some() {
            "completed"
        } else {
            "in-progress"
        }),
    );
    q.insert("subject".into(), json!({ "reference": ctx.patient }));
    let authored = submitted
        .clone()
        .or_else(|| inst.assigned_dt.as_deref().and_then(datetime));
    set(&mut q, "authored", authored.map(|a| json!(a)));
    q.insert("source".into(), json!({ "reference": ctx.patient }));

    let mut answers: BTreeMap<i64, Vec<&FormAnswer>> = BTreeMap::new();
    for a in hp.answers.iter().filter(|a| a.del != Some(true)) {
        if let Some(qid) = a.question_id {
            answers.entry(qid).or_default().push(a);
        }
    }
    let mut options: BTreeMap<i64, Vec<&FormOption>> = BTreeMap::new();
    for o in &hp.question_answer_options {
        if let Some(qid) = o.question_id {
            options.entry(qid).or_default().push(o);
        }
    }
    let mut questions: Vec<&FormQuestion> = hp.questions.iter().filter(|q| q.id.is_some()).collect();
    questions.sort_by_key(|q| (q.display_order.unwrap_or(i64::MAX), q.id));
    let mut sections: Vec<_> = hp.sections.iter().filter(|s| s.id.is_some()).collect();
    sections.sort_by_key(|s| (s.display_order.unwrap_or(i64::MAX), s.id));

    let item_of = |question: &FormQuestion| -> Option<Value> {
        let qid = question.id?;
        let given = answers.get(&qid)?;
        let opts = options.get(&qid).map(Vec::as_slice).unwrap_or(&[]);
        let mut rows: Vec<&&FormAnswer> = given.iter().collect();
        rows.sort_by_key(|a| (a.row.unwrap_or(0), a.id));
        let values: Vec<Value> = rows
            .iter()
            .flat_map(|a| answer_texts(a, opts))
            .map(|t| json!({ "valueString": t }))
            .collect();
        if values.is_empty() {
            return None;
        }
        let label = [&question.label, &question.label_right]
            .iter()
            .filter_map(|l| text_of(l))
            .collect::<Vec<_>>()
            .join(" ");
        let mut item = Map::new();
        item.insert("linkId".into(), json!(format!("question-{qid}")));
        if !label.is_empty() {
            item.insert("text".into(), json!(label));
        }
        item.insert("answer".into(), Value::Array(values));
        Some(Value::Object(item))
    };

    let mut items = Vec::new();
    let mut placed = std::collections::BTreeSet::new();
    for s in &sections {
        let sid = s.id.expect("filtered above");
        let inner: Vec<Value> = questions
            .iter()
            .filter(|q| q.section_id == Some(sid))
            .filter_map(|q| {
                placed.insert(q.id);
                item_of(q)
            })
            .collect();
        if inner.is_empty() {
            continue;
        }
        let mut group = Map::new();
        group.insert("linkId".into(), json!(format!("section-{sid}")));
        set(&mut group, "text", text_of(&s.name).map(|n| json!(n)));
        group.insert("item".into(), Value::Array(inner));
        items.push(Value::Object(group));
    }
    // Questions in no section the form lists.
    items.extend(
        questions
            .iter()
            .filter(|q| !placed.contains(&q.id))
            .filter_map(|q| item_of(q)),
    );
    if !items.is_empty() {
        let template = hp.template.as_ref().and_then(|t| text_of(&t.name)).or_else(|| {
            hp.templates
                .iter()
                .find(|t| t.id.is_some() && t.id == inst.template_id)
                .and_then(|t| text_of(&t.name))
        });
        let mut root = Map::new();
        root.insert(
            "linkId".into(),
            json!(format!("form-{}", inst.template_id.unwrap_or(0))),
        );
        set(&mut root, "text", template.map(|t| json!(t)));
        root.insert("item".into(), Value::Array(items));
        q.insert("item".into(), json!([Value::Object(root)]));
    }
    Some(Value::Object(q))
}

/// What an answer says: each option it names, by its text, else its own
/// text, and any "other" text beside it.
pub fn answer_texts(a: &FormAnswer, options: &[&FormOption]) -> Vec<String> {
    let mut out = Vec::new();
    let raw = text_of(&a.answer_text_full)
        .or_else(|| text_of(&a.answer_text))
        .or_else(|| text_of(&a.answer_num));
    if let Some(raw) = raw {
        let named: Option<Vec<String>> = if options.is_empty() {
            None
        } else {
            raw.split([',', '|', ';'])
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| {
                    options
                        .iter()
                        .find(|o| o.id.map(|i| i.to_string()).as_deref() == Some(p) || o.value.as_deref() == Some(p))
                        .and_then(|o| text_of(&o.text))
                })
                .collect()
        };
        match named {
            Some(texts) if !texts.is_empty() => out.extend(texts),
            _ => out.push(raw),
        }
    }
    if let Some(other) = text_of(&a.answer_text_other) {
        if !out.contains(&other) {
            out.push(other);
        }
    }
    out
}

/// A document of the portal's own JSON: the treatment calendar, whose
/// lists fertemr keeps as the portal sends them.
pub fn json_document(ctx: &Ctx, id: &str, title: &str, date: Option<&str>) -> Value {
    let mut d = Map::new();
    d.insert("resourceType".into(), json!("DocumentReference"));
    d.insert("id".into(), json!(id));
    d.insert("meta".into(), meta());
    d.insert("status".into(), json!("current"));
    d.insert("type".into(), json!({ "text": title }));
    d.insert("subject".into(), json!({ "reference": ctx.patient }));
    set(&mut d, "date", date.and_then(instant).map(|s| json!(s)));
    d.insert("custodian".into(), json!({ "display": ctx.clinic }));
    d.insert("description".into(), json!(title));
    d.insert(
        "content".into(),
        json!([{ "attachment": { "contentType": "application/json", "url": format!("Binary/{id}"), "title": title } }]),
    );
    Value::Object(d)
}

/// A Binary: the file's bytes, base64.
pub fn binary(id: &str, content_type: &str, bytes: &[u8]) -> Value {
    json!({
        "resourceType": "Binary",
        "id": id,
        "meta": meta(),
        "contentType": content_type,
        "data": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FormInstance;

    fn ctx() -> Ctx {
        Ctx {
            origin: "https://portal-someclinic.fertemr.com".into(),
            patient: "Patient/1".into(),
            clinic: "Some Clinic".into(),
        }
    }

    #[test]
    fn a_decimal_keeps_its_spelling() {
        let doc = Doc::new();
        let v = json!({ "resourceType": "Observation", "valueQuantity": { "value": doc.decimal("1.50").unwrap(), "unit": "ng/mL" } });
        let raw = doc.finish(&v).unwrap();
        assert!(raw.raw.as_str().contains(r#""value":1.50,"#), "{}", raw.raw.as_str());
        for bad in ["01", "1.", ".5", "1e3", "", "-", "1,5", "+1"] {
            assert!(doc.decimal(bad).is_none(), "{bad}");
        }
        assert!(doc.decimal("-0.25").is_some());
    }

    #[test]
    fn dates_keep_a_time_only_with_its_zone() {
        assert_eq!(
            datetime("2020-01-02T09:30:00-08:00").as_deref(),
            Some("2020-01-02T09:30:00-08:00")
        );
        assert_eq!(
            datetime("2020-01-02T09:30:00.0000001+00:00").as_deref(),
            Some("2020-01-02T09:30:00.0000001+00:00")
        );
        assert_eq!(
            datetime("2020-01-02T09:30:00Z").as_deref(),
            Some("2020-01-02T09:30:00Z")
        );
        assert_eq!(
            datetime("2020-01-02T09:30:00-0800").as_deref(),
            Some("2020-01-02T09:30:00-08:00")
        );
        assert_eq!(datetime("2020-01-02T09:30:00").as_deref(), Some("2020-01-02"));
        assert_eq!(datetime("2020-01-02").as_deref(), Some("2020-01-02"));
        assert_eq!(datetime("1/2/2020 09:30 AM").as_deref(), Some("2020-01-02"));
        assert_eq!(datetime("12/31/2020").as_deref(), Some("2020-12-31"));
        assert_eq!(datetime("1901-01-01T00:00:00"), None);
        assert_eq!(datetime("0001-01-01T00:00:00"), None);
        assert_eq!(datetime("13/40/2020"), None);
        assert_eq!(datetime("soon"), None);
        assert_eq!(instant("2020-01-02T09:30:00"), None);
        assert_eq!(day("2020-01-02T23:30:00-08:00").as_deref(), Some("2020-01-02"));
    }

    #[test]
    fn a_visit_status_reads_as_fhir_and_keeps_its_words() {
        let v = Visit {
            id: Some(7),
            visit_date: Some("2020-01-02T09:00:00-08:00".into()),
            status: Some("Checked Out".into()),
            appt_duration: Some("30".into()),
            appt_duration_units: Some("Minutes".into()),
            facility: Some("Main".into()),
            city: Some("Example City".into()),
            ..Default::default()
        };
        let e = encounter(&ctx(), &v, "2026-01-01").unwrap();
        assert_eq!(e["id"], "visit-7");
        assert_eq!(e["status"], "finished");
        assert_eq!(e["extension"][0]["valueString"], "Checked Out");
        assert_eq!(e["length"]["value"], 30);
        assert_eq!(e["location"][0]["location"]["display"], "Main, Example City");
        let later = Visit {
            id: Some(8),
            visit_date: Some("2030-01-02T09:00:00-08:00".into()),
            ..Default::default()
        };
        assert_eq!(encounter(&ctx(), &later, "2026-01-01").unwrap()["status"], "planned");
        assert_eq!(encounter(&ctx(), &Visit::default(), "2026-01-01"), None);
    }

    #[test]
    fn a_result_is_a_quantity_only_when_it_is_a_number() {
        let doc = Doc::new();
        let r = LabResult {
            lab_result_id: Some(5),
            test_name: Some("Example analyte".into()),
            test_code: Some("EX1".into()),
            result_value: Some("<0.50".into()),
            result_unit: Some("ng/mL".into()),
            result_abnormal: Some("L".into()),
            lab_status_str: Some("Final".into()),
            specimen_collection_date: Some("2020-01-02T08:00:00-08:00".into()),
            ..Default::default()
        };
        let o = lab_observation(&ctx(), &doc, &r).unwrap();
        let raw = doc.finish(&o).unwrap();
        assert!(raw.raw.as_str().contains(r#""value":0.50"#), "{}", raw.raw.as_str());
        assert_eq!(o["valueQuantity"]["comparator"], "<");
        assert_eq!(o["status"], "final");
        assert_eq!(o["interpretation"][0]["coding"][0]["code"], "L");
        let text = LabResult {
            lab_result_id: Some(6),
            result_value: Some("Positive".into()),
            ..Default::default()
        };
        let o = lab_observation(&ctx(), &doc, &text).unwrap();
        assert_eq!(o["valueString"], "Positive");
        assert_eq!(o["status"], "unknown");
        let none = LabResult {
            order_id: Some(1),
            lab_test_id: Some(2),
            ..Default::default()
        };
        let o = lab_observation(&ctx(), &doc, &none).unwrap();
        assert_eq!(o["id"], "lab-1-2");
        assert!(o.get("dataAbsentReason").is_some());
    }

    #[test]
    fn an_answer_naming_an_option_is_its_text() {
        let opt = FormOption {
            id: Some(11),
            question_id: Some(1),
            text: Some("Yes".into()),
            value: Some("Y".into()),
        };
        let opt2 = FormOption {
            id: Some(12),
            question_id: Some(1),
            text: Some("No".into()),
            value: Some("N".into()),
        };
        let opts = [&opt, &opt2];
        let a = |t: &str| FormAnswer {
            answer_text: Some(t.into()),
            ..Default::default()
        };
        assert_eq!(answer_texts(&a("11"), &opts), ["Yes"]);
        assert_eq!(answer_texts(&a("Y"), &opts), ["Yes"]);
        assert_eq!(answer_texts(&a("11,12"), &opts), ["Yes", "No"]);
        assert_eq!(answer_texts(&a("maybe"), &opts), ["maybe"]);
        assert_eq!(answer_texts(&a("free text"), &[]), ["free text"]);
        let other = FormAnswer {
            answer_text: Some("11".into()),
            answer_text_other: Some("detail".into()),
            ..Default::default()
        };
        assert_eq!(answer_texts(&other, &opts), ["Yes", "detail"]);
    }

    #[test]
    fn a_form_groups_answered_questions_by_section() {
        let hp: HealthProfile = serde_json::from_value(json!({
            "Instance": { "ID": 3, "HPTemplateID": 9, "SubmitDt": "2020-01-02T10:00:00-08:00" },
            "Template": { "ID": 9, "Name": "Example intake" },
            "Sections": [{ "ID": 1, "Name": "First", "DisplayOrder": 2 }, { "ID": 2, "Name": "Empty", "DisplayOrder": 1 }],
            "Questions": [
                { "ID": 10, "HPSectionID": 1, "LabelL": "Asked", "DisplayOrder": 1 },
                { "ID": 11, "HPSectionID": 2, "LabelL": "Unanswered", "DisplayOrder": 1 }
            ],
            "Answers": [{ "ID": 100, "HPQuestionID": 10, "AnswerText": "An answer" }],
            "QuestionAnswerOptions": []
        }))
        .unwrap();
        let q = questionnaire_response(&ctx(), &hp).unwrap();
        assert_eq!(q["id"], "form-3");
        assert_eq!(q["status"], "completed");
        let root = &q["item"][0];
        assert_eq!(root["text"], "Example intake");
        assert_eq!(root["item"].as_array().unwrap().len(), 1);
        assert_eq!(root["item"][0]["text"], "First");
        assert_eq!(root["item"][0]["item"][0]["answer"][0]["valueString"], "An answer");
        let open = HealthProfile {
            instance: Some(FormInstance {
                id: Some(4),
                ..Default::default()
            }),
            ..Default::default()
        };
        let q = questionnaire_response(&ctx(), &open).unwrap();
        assert_eq!(q["status"], "in-progress");
        assert!(q.get("item").is_none());
    }

    #[test]
    fn an_html_body_reads_as_text() {
        let m = Message {
            msg_id: Some(1),
            body: Some("<p>Line one</p><p>Two &amp; three</p>".into()),
            body_is_html: Some(true),
            ..Default::default()
        };
        let c = communication(&ctx(), &m, Some(9), &["att-4".into()]).unwrap();
        assert_eq!(c["payload"][0]["contentString"], "Line one\n\nTwo & three");
        assert_eq!(
            c["payload"][1]["contentReference"]["reference"],
            "DocumentReference/att-4"
        );
        assert_eq!(c["inResponseTo"][0]["reference"], "Communication/msg-9");
    }
}
