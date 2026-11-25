//! The portal API's answers, as far as fertemr reads them.
//!
//! Every answer but the sign-in is an envelope, `{"c": <code>, "t": <text>,
//! "d": <data>, "r": ...}`, with `c` 0 for success. Field names are the
//! portal's own (PascalCase). Every field is optional and lenient about
//! its JSON type (a number where a string was seen, or the reverse), so a
//! field the portal drops or retypes empties one value instead of failing
//! the whole read; `tests/fixture_shapes.rs` pins the fields read here.

use serde::de::{self, Deserializer};
use serde::Deserialize;
use serde_json::Value;

/// A string, number or boolean as text; null and absent are `None`, and so
/// is an empty or all-blank string.
pub fn text<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => Some(s),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    })
}

/// A whole number, from a number or a numeric string.
pub fn int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

/// A boolean, from a boolean, `0`/`1` or `"true"`/`"false"`.
pub fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Bool(b) => Some(b),
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

/// A list, empty when null or absent.
pub fn list<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// The envelope every API answer but the sign-in comes in.
#[derive(Debug, Deserialize)]
pub struct Envelope {
    #[serde(default, deserialize_with = "int")]
    pub c: Option<i64>,
    #[serde(default, deserialize_with = "text")]
    pub t: Option<String>,
    #[serde(default)]
    pub d: Value,
}

/// `User/LoginAttempt` and `User/LoginValidate`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
#[allow(non_snake_case)]
pub struct LoginAnswer {
    #[serde(deserialize_with = "flag")]
    pub success: Option<bool>,
    #[serde(deserialize_with = "text")]
    pub err: Option<String>,
    #[serde(rename = "ppUserId", deserialize_with = "int")]
    pub pp_user_id: Option<i64>,
    #[serde(rename = "ptId", deserialize_with = "int")]
    pub pt_id: Option<i64>,
    #[serde(rename = "uniqueSessionID", deserialize_with = "text")]
    pub session_id: Option<String>,
    #[serde(rename = "Username", deserialize_with = "text")]
    pub username: Option<String>,
    #[serde(rename = "FullName", deserialize_with = "text")]
    pub full_name: Option<String>,
    #[serde(rename = "FirstName", deserialize_with = "text")]
    pub first_name: Option<String>,
    #[serde(rename = "LastName", deserialize_with = "text")]
    pub last_name: Option<String>,
    #[serde(rename = "AcceptedLatestTOS", deserialize_with = "flag")]
    pub accepted_latest_tos: Option<bool>,
    #[serde(rename = "Timezone", deserialize_with = "text")]
    pub timezone: Option<String>,
    // An error answer is an envelope instead.
    #[serde(deserialize_with = "int")]
    pub c: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub t: Option<String>,
}

/// A clinic portal's public `config.json`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct PortalConfig {
    #[serde(rename = "clientCode", deserialize_with = "text")]
    pub client_code: Option<String>,
    #[serde(rename = "baseUrl", deserialize_with = "text")]
    pub base_url: Option<String>,
    #[serde(rename = "clientNameFull", deserialize_with = "text")]
    pub name_full: Option<String>,
    #[serde(rename = "clientNameShort", deserialize_with = "text")]
    pub name_short: Option<String>,
    #[serde(deserialize_with = "text")]
    pub environment: Option<String>,
}

/// `d` of a list call: `{"Results": [...]}`.
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
pub struct Results<T> {
    #[serde(rename = "Results", default = "Vec::new", deserialize_with = "list")]
    pub results: Vec<T>,
}

/// `PPVisits/List`: an appointment, past or upcoming.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Visit {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub visit_date: Option<String>,
    #[serde(rename = "Type", deserialize_with = "text")]
    pub kind: Option<String>,
    #[serde(deserialize_with = "text")]
    pub appt_duration: Option<String>,
    #[serde(deserialize_with = "text")]
    pub appt_duration_units: Option<String>,
    #[serde(deserialize_with = "text")]
    pub status: Option<String>,
    #[serde(deserialize_with = "text")]
    pub facility: Option<String>,
    #[serde(deserialize_with = "text")]
    pub addr1: Option<String>,
    #[serde(deserialize_with = "text")]
    pub addr2: Option<String>,
    #[serde(deserialize_with = "text")]
    pub city: Option<String>,
    #[serde(deserialize_with = "text")]
    pub state: Option<String>,
    #[serde(deserialize_with = "text")]
    pub zip: Option<String>,
    #[serde(deserialize_with = "text")]
    pub country: Option<String>,
}

/// `PPLab/List` and `PPLab/GetByID`: one result line of an order.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct LabResult {
    #[serde(rename = "OrderID", deserialize_with = "int")]
    pub order_id: Option<i64>,
    #[serde(rename = "LabTestID", deserialize_with = "int")]
    pub lab_test_id: Option<i64>,
    #[serde(rename = "LabResultID", deserialize_with = "int")]
    pub lab_result_id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub source_table: Option<String>,
    #[serde(deserialize_with = "text")]
    pub test_code: Option<String>,
    #[serde(deserialize_with = "text")]
    pub test_name: Option<String>,
    #[serde(deserialize_with = "text")]
    pub test_description: Option<String>,
    #[serde(deserialize_with = "text")]
    pub category: Option<String>,
    #[serde(deserialize_with = "text")]
    pub observation_text: Option<String>,
    #[serde(deserialize_with = "text")]
    pub lab_location_str: Option<String>,
    #[serde(rename = "AccessionID", deserialize_with = "text")]
    pub accession_id: Option<String>,
    #[serde(deserialize_with = "text")]
    pub test_request_date: Option<String>,
    #[serde(deserialize_with = "text")]
    pub specimen_collection_date: Option<String>,
    #[serde(deserialize_with = "text")]
    pub assay_completion_date: Option<String>,
    #[serde(deserialize_with = "text")]
    pub report_date: Option<String>,
    #[serde(deserialize_with = "text")]
    pub lab_status_str: Option<String>,
    #[serde(deserialize_with = "text")]
    pub result_value: Option<String>,
    #[serde(deserialize_with = "text")]
    pub result_unit: Option<String>,
    #[serde(deserialize_with = "text")]
    pub result_ref_range: Option<String>,
    #[serde(deserialize_with = "text")]
    pub result_abnormal: Option<String>,
    #[serde(deserialize_with = "text")]
    pub result_note: Option<String>,
    #[serde(deserialize_with = "text")]
    pub value_out: Option<String>,
    #[serde(deserialize_with = "text")]
    pub value_out_flag: Option<String>,
    #[serde(deserialize_with = "text")]
    pub ref_range: Option<String>,
    #[serde(deserialize_with = "text")]
    pub value_unit: Option<String>,
    #[serde(deserialize_with = "text")]
    pub lab_user_note: Option<String>,
    #[serde(rename = "ResultPDFID", deserialize_with = "int")]
    pub result_pdf_id: Option<i64>,
    #[serde(rename = "ResultPDFName", deserialize_with = "text")]
    pub result_pdf_name: Option<String>,
    #[serde(rename = "RecordTypeID", deserialize_with = "int")]
    pub record_type_id: Option<i64>,
}

/// `PPFiles/List`: a file the clinic shared.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct FileItem {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub file_name: Option<String>,
    #[serde(deserialize_with = "text")]
    pub category: Option<String>,
    #[serde(deserialize_with = "text")]
    pub sub_category: Option<String>,
    #[serde(deserialize_with = "text")]
    pub download_link: Option<String>,
    #[serde(deserialize_with = "flag")]
    pub is_external: Option<bool>,
}

/// `PPMessaging/List`: `d`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MessageList {
    #[serde(deserialize_with = "int")]
    pub total_messages: Option<i64>,
    #[serde(deserialize_with = "int")]
    pub total_pages: Option<i64>,
    #[serde(deserialize_with = "list")]
    pub messages: Vec<Message>,
}

/// One secure message.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct Message {
    #[serde(rename = "MsgID", deserialize_with = "int")]
    pub msg_id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub subject: Option<String>,
    #[serde(deserialize_with = "text")]
    pub body: Option<String>,
    #[serde(deserialize_with = "text")]
    pub plain_text_body: Option<String>,
    #[serde(rename = "BodyIsHTML", deserialize_with = "flag")]
    pub body_is_html: Option<bool>,
    #[serde(deserialize_with = "text")]
    pub dt_sent_or_received: Option<String>,
    #[serde(deserialize_with = "text")]
    pub from_user_display: Option<String>,
    #[serde(deserialize_with = "text")]
    pub from_group_display: Option<String>,
    #[serde(deserialize_with = "text")]
    pub from_patient_display: Option<String>,
    #[serde(deserialize_with = "text")]
    pub to_user_display: Option<String>,
    #[serde(deserialize_with = "text")]
    pub to_group_display: Option<String>,
    #[serde(deserialize_with = "text")]
    pub to_patient_display: Option<String>,
    #[serde(deserialize_with = "flag")]
    pub draft: Option<bool>,
    #[serde(deserialize_with = "flag")]
    pub trashed: Option<bool>,
    #[serde(deserialize_with = "list")]
    pub attachments: Vec<Attachment>,
    /// The replies, each a message of its own.
    #[serde(deserialize_with = "list")]
    pub messages_in_thread: Vec<Message>,
}

/// A message's attachment.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct Attachment {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub file_name: Option<String>,
}

/// `PPEmbryology/List`: `d`, one set of records with the columns the
/// clinic shows (`Show*`).
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Embryology {
    pub set: EmbryologySet,
    #[serde(deserialize_with = "list")]
    pub records: Vec<EmbryoRecord>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct EmbryologySet {
    #[serde(deserialize_with = "text")]
    pub publish_date_display: Option<String>,
}

/// One embryo (or oocyte) record. The dates are as the clinic typed them.
#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct EmbryoRecord {
    #[serde(rename = "RecordID", deserialize_with = "int")]
    pub record_id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub embryo_num: Option<String>,
    #[serde(deserialize_with = "text")]
    pub record_day: Option<String>,
    #[serde(deserialize_with = "text")]
    pub stage: Option<String>,
    #[serde(deserialize_with = "text")]
    pub grade: Option<String>,
    #[serde(rename = "PGTA", deserialize_with = "text")]
    pub pgta: Option<String>,
    #[serde(rename = "PGTOther", deserialize_with = "text")]
    pub pgt_other: Option<String>,
    #[serde(deserialize_with = "text")]
    pub sex: Option<String>,
    #[serde(deserialize_with = "text")]
    pub interpretation: Option<String>,
    #[serde(deserialize_with = "text")]
    pub record_status: Option<String>,
    #[serde(deserialize_with = "text")]
    pub ooc_source: Option<String>,
    #[serde(deserialize_with = "text")]
    pub sperm_source: Option<String>,
    #[serde(deserialize_with = "text")]
    pub device_num: Option<String>,
    #[serde(deserialize_with = "text")]
    pub freeze_date: Option<String>,
    #[serde(deserialize_with = "text")]
    pub xfer_date: Option<String>,
    #[serde(deserialize_with = "text")]
    pub discard_date: Option<String>,
    #[serde(deserialize_with = "text")]
    pub comment: Option<String>,
}

/// `PPHP/ListInstances` and `PPHP/Instance`: intake forms ("health
/// profiles"), each an instance of a template.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct HealthProfile {
    pub instance: Option<FormInstance>,
    #[serde(deserialize_with = "list")]
    pub instances: Vec<FormInstance>,
    pub template: Option<FormTemplate>,
    #[serde(deserialize_with = "list")]
    pub templates: Vec<FormTemplate>,
    #[serde(deserialize_with = "list")]
    pub answers: Vec<FormAnswer>,
    #[serde(deserialize_with = "list")]
    pub sections: Vec<FormSection>,
    #[serde(deserialize_with = "list")]
    pub questions: Vec<FormQuestion>,
    #[serde(deserialize_with = "list")]
    pub question_answer_options: Vec<FormOption>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct FormInstance {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(rename = "HPTemplateID", deserialize_with = "int")]
    pub template_id: Option<i64>,
    #[serde(rename = "HPSubmissionStatusID", deserialize_with = "int")]
    pub submission_status_id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub assigned_dt: Option<String>,
    #[serde(deserialize_with = "text")]
    pub submit_dt: Option<String>,
    #[serde(deserialize_with = "flag")]
    pub del: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct FormTemplate {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub name: Option<String>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct FormAnswer {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(rename = "HPQuestionID", deserialize_with = "int")]
    pub question_id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub answer_text: Option<String>,
    #[serde(deserialize_with = "text")]
    pub answer_text_full: Option<String>,
    #[serde(deserialize_with = "text")]
    pub answer_text_other: Option<String>,
    #[serde(deserialize_with = "text")]
    pub answer_num: Option<String>,
    #[serde(rename = "MultiRecordRowID", deserialize_with = "int")]
    pub row: Option<i64>,
    #[serde(deserialize_with = "flag")]
    pub del: Option<bool>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct FormSection {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub name: Option<String>,
    #[serde(deserialize_with = "int")]
    pub display_order: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct FormQuestion {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(rename = "HPSectionID", deserialize_with = "int")]
    pub section_id: Option<i64>,
    #[serde(rename = "LabelL", deserialize_with = "text")]
    pub label: Option<String>,
    #[serde(rename = "LabelR", deserialize_with = "text")]
    pub label_right: Option<String>,
    #[serde(deserialize_with = "int")]
    pub display_order: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Clone)]
#[serde(default, rename_all = "PascalCase")]
pub struct FormOption {
    #[serde(rename = "ID", deserialize_with = "int")]
    pub id: Option<i64>,
    #[serde(rename = "HPQuestionID", deserialize_with = "int")]
    pub question_id: Option<i64>,
    #[serde(deserialize_with = "text")]
    pub text: Option<String>,
    #[serde(deserialize_with = "text")]
    pub value: Option<String>,
}

/// `PPCalendar/List`: `d`. The calendar's own lists are kept as the portal
/// sends them (`tests/fixtures/portal/calendar.json`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Calendar {
    #[serde(rename = "PtMRN", deserialize_with = "text")]
    pub mrn: Option<String>,
    #[serde(rename = "PtDOB", deserialize_with = "text")]
    pub dob: Option<String>,
    #[serde(rename = "PtName", deserialize_with = "text")]
    pub name: Option<String>,
    #[serde(rename = "LastUpdate", deserialize_with = "text")]
    pub last_update: Option<String>,
    #[serde(rename = "CalCycles", deserialize_with = "list")]
    pub cycles: Vec<Value>,
    #[serde(rename = "CalMasters", deserialize_with = "list")]
    pub masters: Vec<Value>,
    #[serde(rename = "CalEvents", deserialize_with = "list")]
    pub events: Vec<Value>,
    #[serde(rename = "CalEventsUp", deserialize_with = "list")]
    pub events_up: Vec<Value>,
    #[serde(rename = "CurrentCal")]
    pub current: Value,
}

impl Calendar {
    /// Whether the calendar holds anything beyond the patient's own header.
    pub fn is_empty(&self) -> bool {
        self.cycles.is_empty()
            && self.masters.is_empty()
            && self.events.is_empty()
            && self.events_up.is_empty()
            && self.current.is_null()
    }
}

/// Reads `T` out of `value`, naming `what` (never a value) when it does not
/// read.
pub fn read<T: de::DeserializeOwned>(value: Value, what: &str) -> Result<T, pk_cli_core::CliError> {
    serde_json::from_value(value).map_err(|e| {
        pk_cli_core::CliError::Upstream(format!(
            "the portal's answer for {what} is not the shape fertemr reads ({})",
            fault(&e)
        ))
    })
}

/// What kind of fault a serde error is, without the value serde quotes.
fn fault(e: &serde_json::Error) -> &'static str {
    use serde_json::error::Category;
    match e.classify() {
        Category::Io => "an I/O error",
        Category::Syntax => "not JSON",
        Category::Data => "a field of another type",
        Category::Eof => "cut short",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lenient_fields_read_numbers_strings_and_blanks() {
        let v: Visit = serde_json::from_value(json!({
            "ID": "12", "VisitDate": "2020-01-02T09:00:00", "Type": 7, "Status": "  ", "Zip": null
        }))
        .unwrap();
        assert_eq!(v.id, Some(12));
        assert_eq!(v.kind.as_deref(), Some("7"));
        assert_eq!(v.status, None);
        assert_eq!(v.zip, None);
    }

    #[test]
    fn a_null_list_is_empty() {
        let r: Results<FileItem> = serde_json::from_value(json!({"Results": null})).unwrap();
        assert!(r.results.is_empty());
        let m: MessageList = serde_json::from_value(json!({"Messages": null})).unwrap();
        assert!(m.messages.is_empty());
    }

    #[test]
    fn a_wrong_shape_names_no_value() {
        let e = read::<Results<FileItem>>(json!({"Results": "Jane Doe"}), "files").unwrap_err();
        assert!(!e.to_string().contains("Jane"), "{e}");
        assert_eq!(e.exit_code(), 5);
    }
}
