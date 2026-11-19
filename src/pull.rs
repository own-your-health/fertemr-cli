//! Reading a patient's records from the portal, and writing them as a
//! records-stream.
//!
//! The portal is read through [`Reader`], one read or download per call, so
//! a test can answer from fixtures. Every list is one read: the portal's
//! own app asks for each list whole, with no date or page parameter, except
//! messages, which it pages 1000 at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;

use pk_cli_core::CliError;
use pk_med_records::stream::skip;
use pk_med_records::{Finished, Header, RecordsWriter, Skip};
use serde_json::{json, Value};

use crate::fhir::{self, Ctx, Doc};
use crate::model::{
    self, Calendar, Embryology, FileItem, HealthProfile, LabResult, Message, MessageList, Results, Visit,
};

/// The portal, as a pull reads it.
pub trait Reader {
    /// One read: the envelope's `d`.
    fn read(&self, path: &str, body: &Value) -> Result<Value, CliError>;
    /// One download: the file's bytes.
    fn download(&self, path: &str, body: &Value) -> Result<Vec<u8>, CliError>;
}

/// The list call's body the portal's app sends.
pub fn list_body() -> Value {
    json!({ "s": "" })
}

/// The message folders a pull reads. Drafts are the patient's unsent text.
pub const FOLDERS: &[&str] = &["Inbox", "Sent", "Archive"];
/// The page size the portal's app asks for.
const PAGE_SIZE: i64 = 1000;
/// Pages read per folder at most.
const PAGE_CAP: i64 = 20;

/// `PPMessaging/List`'s body for a folder and page, as the app sends it.
pub fn messages_body(folder: &str, page: i64) -> Value {
    json!({
        "Folder": folder,
        "DtFrom": "1901-01-01T00:00:00.000",
        "DtTo": "1901-01-01T00:00:00.000",
        "UnreadOnly": false,
        "NotReplied": false,
        "SearchTerm": "",
        "TotalMessages": 0,
        "TotalPages": 0,
        "Page": page,
        "PageSize": PAGE_SIZE,
        "Messages": [],
        "IsReadArchTrash": false,
        "Options_PageSize": "[]",
        "Selected_PageSize": "[]",
    })
}

/// A file the portal serves: what `documents list` shows and the download
/// call that fetches it.
#[derive(Debug, Clone)]
pub struct Fetchable {
    /// `file-<id>`, `labpdf-<id>-<type>` or `att-<id>`.
    pub id: String,
    pub name: String,
    pub category: &'static str,
    /// The day it is dated, when the portal dates it.
    pub date: Option<String>,
    pub path: &'static str,
    pub body: Value,
}

/// Everything the documents commands list: the clinic's files (external
/// links left out: they are not the portal's to serve), lab report PDFs and
/// message attachments. A list the portal does not offer, or fails to
/// answer, is left out and said in `missing`; a session it no longer
/// accepts stops the listing.
pub fn documents(r: &dyn Reader) -> Result<(Vec<Fetchable>, Vec<String>), CliError> {
    let mut out = Vec::new();
    let mut missing = Vec::new();
    let mut tolerate = |answer: Result<Value, CliError>, what: &str| -> Result<Option<Value>, CliError> {
        match answer {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.exit_code() == 4 => {
                missing.push(format!("the portal does not offer {what} to this patient"));
                Ok(None)
            }
            Err(e) if e.exit_code() == 5 => {
                missing.push(format!("{what} could not be read: {}", e.message()));
                Ok(None)
            }
            Err(e) => Err(e),
        }
    };
    let files = tolerate(r.read("PPFiles/List", &list_body()), "files")?;
    let labs = tolerate(r.read("PPLab/List", &list_body()), "lab results")?;
    let msgs = match messages(r) {
        Ok(m) => m,
        Err(e) => {
            tolerate(Err(e), "messages")?;
            Vec::new()
        }
    };
    if let Some(d) = files {
        let files: Results<FileItem> = model::read(d, "files")?;
        for f in files.results {
            let Some(id) = f.id else { continue };
            if f.is_external == Some(true) {
                continue;
            }
            out.push(Fetchable {
                id: format!("file-{id}"),
                name: f.file_name.clone().unwrap_or_else(|| format!("file-{id}")),
                category: "file",
                date: None,
                path: "PPFiles/DownloadFile",
                body: json!({ "FileID": id }),
            });
        }
    }
    if let Some(d) = labs {
        let labs: Results<LabResult> = model::read(d, "lab results")?;
        let mut seen = BTreeSet::new();
        for l in labs.results {
            if let Some(f) = lab_pdf(&l) {
                if seen.insert(f.id.clone()) {
                    out.push(f);
                }
            }
        }
    }
    for m in &msgs {
        for (msg, a) in attachments_of(m) {
            let Some(id) = a.id else { continue };
            out.push(Fetchable {
                id: format!("att-{id}"),
                name: a.file_name.clone().unwrap_or_else(|| format!("attachment-{id}")),
                category: "message-attachment",
                date: msg.dt_sent_or_received.as_deref().and_then(fhir::day),
                path: "PPMessaging/DownloadAttachment",
                body: json!({ "FileID": id }),
            });
        }
    }
    let mut seen = BTreeSet::new();
    out.retain(|f| seen.insert(f.id.clone()));
    Ok((out, missing))
}

/// A lab result's report PDF, as the portal's download call names it.
fn lab_pdf(l: &LabResult) -> Option<Fetchable> {
    let pdf = l.result_pdf_id.filter(|id| *id > 0)?;
    let kind = l.record_type_id.unwrap_or(0);
    Some(Fetchable {
        id: format!("labpdf-{pdf}-{kind}"),
        name: l
            .result_pdf_name
            .clone()
            .unwrap_or_else(|| format!("lab-report-{pdf}.pdf")),
        category: "lab-report",
        date: l
            .report_date
            .as_deref()
            .or(l.specimen_collection_date.as_deref())
            .and_then(fhir::day),
        path: "PPLab/DownloadFile",
        body: json!({ "FileID": pdf, "RecordTypeID": kind }),
    })
}

/// A message and its replies, flattened, each with the message it answers.
fn thread(m: &Message, parent: Option<i64>, out: &mut Vec<(Message, Option<i64>)>) {
    let mut top = m.clone();
    let replies = std::mem::take(&mut top.messages_in_thread);
    let id = top.msg_id;
    out.push((top, parent));
    for r in &replies {
        thread(r, id.or(parent), out);
    }
}

fn attachments_of(m: &Message) -> Vec<(&Message, &model::Attachment)> {
    let mut out: Vec<(&Message, &model::Attachment)> = m.attachments.iter().map(|a| (m, a)).collect();
    for r in &m.messages_in_thread {
        out.extend(attachments_of(r));
    }
    out
}

/// Every message in [`FOLDERS`], drafts and trashed ones left out, each
/// once.
pub fn messages(r: &dyn Reader) -> Result<Vec<Message>, CliError> {
    Ok(messages_paged(r)?.0)
}

/// [`messages`], and the folders left at [`PAGE_CAP`].
fn messages_paged(r: &dyn Reader) -> Result<(Vec<Message>, Vec<&'static str>), CliError> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut capped = Vec::new();
    for folder in FOLDERS {
        let mut page = 0;
        loop {
            let d = r.read("PPMessaging/List", &messages_body(folder, page))?;
            let list: MessageList = model::read(d, "messages")?;
            let before = seen.len();
            let got = list.messages.len();
            for m in list.messages {
                if m.draft == Some(true) || m.trashed == Some(true) {
                    continue;
                }
                if m.msg_id.is_none_or(|id| seen.insert(id)) {
                    out.push(m);
                }
            }
            page += 1;
            let pages = list.total_pages.unwrap_or(1);
            let more = (got as i64) >= PAGE_SIZE && page < pages.max(1) && seen.len() > before;
            if !more {
                break;
            }
            if page >= PAGE_CAP {
                capped.push(*folder);
                break;
            }
        }
    }
    Ok((out, capped))
}

/// The patient's name and ids, from the stored session.
pub struct Who {
    pub patient_id: String,
    pub full_name: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

/// What a pull found, for the trailer's counts and the stderr summary.
#[derive(Debug, Default)]
pub struct Tally {
    /// Records left out by `--since`.
    pub before_since: u64,
}

/// One list's outcome: written, not offered, or failed (a skip).
fn list_or_skip<W: Write>(
    w: &mut RecordsWriter<W>,
    answer: Result<Value, CliError>,
    resource_type: &str,
    search: &str,
) -> Result<Option<Value>, CliError> {
    match answer {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.exit_code() == 4 => {
            w.skip(Skip::search(
                search,
                skip::UNSUPPORTED,
                format!("the portal does not offer {resource_type} records to this patient"),
            ));
            Ok(None)
        }
        Err(e) if e.exit_code() == 5 => {
            w.skip(Skip::search(search, skip::UPSTREAM, e.message().to_string()));
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// Whether a dated record is kept under `--since`; an undated one always is.
fn keep(since: Option<&str>, day: Option<&str>) -> bool {
    match (since, day) {
        (Some(s), Some(d)) => d >= s,
        _ => true,
    }
}

/// One file, fetched and checked, as a Binary; a failure is a skip.
fn fetch<W: Write>(
    r: &dyn Reader,
    w: &mut RecordsWriter<W>,
    path: &str,
    body: &Value,
    owner: (&str, &str),
    name: Option<&str>,
) -> Result<Option<(Vec<u8>, &'static str)>, CliError> {
    let bytes = match r.download(path, body) {
        Ok(b) => b,
        Err(e) if e.is_auth() => return Err(e),
        Err(e) => {
            let code = if e.exit_code() == 4 {
                skip::NO_FILE
            } else {
                skip::UPSTREAM
            };
            w.skip(Skip::resource(
                owner.0,
                owner.1,
                code,
                format!("its file was not served: {}", e.message()),
            ));
            return Ok(None);
        }
    };
    if bytes.is_empty() {
        w.skip(Skip::resource(
            owner.0,
            owner.1,
            skip::NO_FILE,
            "the portal served an empty file",
        ));
        return Ok(None);
    }
    let ctype = fhir::content_type(name, Some(&bytes));
    let ext = match ctype {
        "application/pdf" => Some("pdf"),
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        _ => None,
    };
    if let Some(ext) = ext {
        if let Err(e) = pk_cli_documents::verify_download(&bytes, Some(ext)) {
            w.skip(Skip::resource(
                owner.0,
                owner.1,
                skip::VERIFY_FAILED,
                e.message().to_string(),
            ));
            return Ok(None);
        }
    }
    Ok(Some((bytes, ctype)))
}

/// The pull: the stream's header, then each list's resources, then the
/// trailer. An error after the header ends the stream with `complete:
/// false` and is handed back beside it; `Err` is a failure to write.
pub fn export<W: Write>(
    r: &dyn Reader,
    out: W,
    header: &Header,
    ctx: &Ctx,
    who: &Who,
    today: &str,
) -> Result<(Finished, Tally, Option<CliError>), CliError> {
    let mut w = RecordsWriter::start(out, header)?;
    let mut tally = Tally::default();
    let result = write_all(r, &mut w, header.since.as_deref(), ctx, who, today, &mut tally);
    let finished_at = crate::app::now_rfc3339();
    let finished = w.finish(finished_at, result.as_ref().err())?;
    Ok((finished, tally, result.err()))
}

fn write_all<W: Write>(
    r: &dyn Reader,
    w: &mut RecordsWriter<W>,
    since: Option<&str>,
    ctx: &Ctx,
    who: &Who,
    today: &str,
    tally: &mut Tally,
) -> Result<(), CliError> {
    let doc = Doc::new();
    let put = |w: &mut RecordsWriter<W>, v: &Value| -> Result<(), CliError> {
        w.resource(&doc.finish(v)?)?;
        Ok(())
    };
    put(
        w,
        &fhir::patient(
            ctx,
            &who.patient_id,
            who.full_name.as_deref(),
            who.first_name.as_deref(),
            who.last_name.as_deref(),
        ),
    )?;

    // Visits.
    if let Some(d) = list_or_skip(w, r.read("PPVisits/List", &list_body()), "visit", "PPVisits/List")? {
        let visits: Results<Visit> = model::read(d, "visits")?;
        for v in &visits.results {
            if !keep(since, v.visit_date.as_deref().and_then(fhir::day).as_deref()) {
                tally.before_since += 1;
                continue;
            }
            if let Some(e) = fhir::encounter(ctx, v, today) {
                put(w, &e)?;
            }
        }
    }

    // Lab results: an Observation per line, a DiagnosticReport per order
    // with its PDF.
    if let Some(d) = list_or_skip(w, r.read("PPLab/List", &list_body()), "lab", "PPLab/List")? {
        let labs: Results<LabResult> = model::read(d, "lab results")?;
        let mut orders: BTreeMap<i64, Vec<&LabResult>> = BTreeMap::new();
        let mut loose = Vec::new();
        for l in &labs.results {
            let dated = l
                .specimen_collection_date
                .as_deref()
                .or(l.test_request_date.as_deref())
                .or(l.report_date.as_deref())
                .and_then(fhir::day);
            if !keep(since, dated.as_deref()) {
                tally.before_since += 1;
                continue;
            }
            match l.order_id {
                Some(o) => orders.entry(o).or_default().push(l),
                None => loose.push(l),
            }
        }
        for l in &loose {
            if let Some(o) = fhir::lab_observation(ctx, &doc, l) {
                put(w, &o)?;
            }
        }
        for (order, lines) in &orders {
            for l in lines {
                if let Some(o) = fhir::lab_observation(ctx, &doc, l) {
                    put(w, &o)?;
                }
            }
            let report_id = format!("lab-order-{order}");
            let pdf = lines.iter().find_map(|l| lab_pdf(l));
            let fetched = match &pdf {
                Some(f) => fetch(r, w, f.path, &f.body, ("DiagnosticReport", &report_id), Some(&f.name))?,
                None => None,
            };
            let named = match (&pdf, &fetched) {
                (Some(f), Some(_)) => Some((f.id.as_str(), Some(f.name.as_str()))),
                _ => None,
            };
            put(w, &fhir::lab_report(ctx, *order, lines, named))?;
            if let (Some(f), Some((bytes, ctype))) = (&pdf, &fetched) {
                put(w, &fhir::binary(&f.id, ctype, bytes))?;
            }
        }
    }

    // Embryology: undated records, kept under --since.
    if let Some(d) = list_or_skip(
        w,
        r.read("PPEmbryology/List", &list_body()),
        "embryology",
        "PPEmbryology/List",
    )? {
        let e: Embryology = model::read(d, "embryology")?;
        let published = e.set.publish_date_display.as_deref();
        for rec in &e.records {
            if let Some(o) = fhir::embryo_observation(ctx, rec, published) {
                put(w, &o)?;
            }
        }
    }

    // Files the clinic shared.
    if let Some(d) = list_or_skip(w, r.read("PPFiles/List", &list_body()), "file", "PPFiles/List")? {
        let files: Results<FileItem> = model::read(d, "files")?;
        for f in &files.results {
            let Some(id) = f.id else { continue };
            let doc_id = format!("file-{id}");
            if f.is_external == Some(true) {
                if let Some(d) = fhir::file_document(ctx, f, None) {
                    put(w, &d)?;
                }
                continue;
            }
            let fetched = fetch(
                r,
                w,
                "PPFiles/DownloadFile",
                &json!({ "FileID": id }),
                ("DocumentReference", &doc_id),
                f.file_name.as_deref(),
            )?;
            let named = fetched.as_ref().map(|(_, ctype)| (doc_id.as_str(), *ctype));
            if let Some(d) = fhir::file_document(ctx, f, named) {
                put(w, &d)?;
            }
            if let Some((bytes, ctype)) = &fetched {
                put(w, &fhir::binary(&doc_id, ctype, bytes))?;
            }
        }
    }

    // Messages, with their replies and attachments.
    match messages_paged(r) {
        Ok((list, capped)) => {
            for folder in capped {
                w.skip(Skip::search(
                    format!("PPMessaging/List?Folder={folder}"),
                    skip::PAGE_CAP,
                    format!("pages past {PAGE_CAP} of {PAGE_SIZE} messages were left"),
                ));
            }
            let mut flat = Vec::new();
            for m in &list {
                thread(m, None, &mut flat);
            }
            let mut seen = BTreeSet::new();
            for (m, parent) in &flat {
                let Some(id) = m.msg_id else { continue };
                if !seen.insert(id) {
                    continue;
                }
                if !keep(since, m.dt_sent_or_received.as_deref().and_then(fhir::day).as_deref()) {
                    tally.before_since += 1;
                    continue;
                }
                let msg_id = format!("msg-{id}");
                let mut docs = Vec::new();
                let mut files = Vec::new();
                for a in &m.attachments {
                    let Some(aid) = a.id else { continue };
                    let att = format!("att-{aid}");
                    let fetched = fetch(
                        r,
                        w,
                        "PPMessaging/DownloadAttachment",
                        &json!({ "FileID": aid }),
                        ("DocumentReference", &att),
                        a.file_name.as_deref(),
                    )?;
                    if let Some((bytes, ctype)) = fetched {
                        docs.push(att);
                        files.push((a.clone(), bytes, ctype));
                    }
                }
                if let Some(c) = fhir::communication(ctx, m, *parent, &docs) {
                    put(w, &c)?;
                }
                for (a, bytes, ctype) in &files {
                    let att = format!("att-{}", a.id.unwrap_or_default());
                    if let Some(d) = fhir::attachment_document(ctx, a, &msg_id, m.dt_sent_or_received.as_deref(), ctype)
                    {
                        put(w, &d)?;
                        put(w, &fhir::binary(&att, ctype, bytes))?;
                    }
                }
            }
        }
        Err(e) if e.exit_code() == 4 => {
            w.skip(Skip::search(
                "PPMessaging/List",
                skip::UNSUPPORTED,
                "the portal does not offer messages to this patient",
            ));
        }
        Err(e) if e.exit_code() == 5 => {
            w.skip(Skip::search(
                "PPMessaging/List",
                skip::UPSTREAM,
                e.message().to_string(),
            ));
        }
        Err(e) => return Err(e),
    }

    // Intake forms ("health profiles"): one read for the list, one per form.
    if let Some(d) = list_or_skip(
        w,
        r.read("PPHP/ListInstances", &list_body()),
        "form",
        "PPHP/ListInstances",
    )? {
        let list: HealthProfile = model::read(d, "forms")?;
        for inst in list.instances.iter().filter(|i| i.del != Some(true)) {
            let Some(id) = inst.id else { continue };
            let dated = inst
                .submit_dt
                .as_deref()
                .or(inst.assigned_dt.as_deref())
                .and_then(fhir::day);
            if !keep(since, dated.as_deref()) {
                tally.before_since += 1;
                continue;
            }
            let answer = r.read("PPHP/Instance", &json!({ "id": id }));
            let search = format!("PPHP/Instance?id={id}");
            let Some(d) = list_or_skip(w, answer, "form", &search)? else {
                continue;
            };
            let mut hp: HealthProfile = model::read(d, "a form")?;
            if hp.template.is_none() {
                hp.templates.clone_from(&list.templates);
            }
            if let Some(q) = fhir::questionnaire_response(ctx, &hp) {
                put(w, &q)?;
            }
        }
    }

    // The treatment calendar, kept as the portal's JSON when it holds any.
    if let Some(d) = list_or_skip(
        w,
        r.read("PPCalendar/List", &list_body()),
        "calendar",
        "PPCalendar/List",
    )? {
        let cal: Calendar = model::read(d.clone(), "the calendar")?;
        if !cal.is_empty() {
            let dated = cal.last_update.as_deref();
            if keep(since, dated.and_then(fhir::day).as_deref()) {
                let bytes =
                    serde_json::to_vec(&d).map_err(|e| CliError::Other(format!("writing the calendar: {e}")))?;
                put(w, &fhir::json_document(ctx, "calendar", "Treatment calendar", dated))?;
                put(w, &fhir::binary("calendar", "application/json", &bytes))?;
            } else {
                tally.before_since += 1;
            }
        }
    }
    Ok(())
}

/// The resource types a pull writes undated, so `--since` leaves them in.
pub const SINCE_UNBOUNDED: &[&str] = &["Patient", "Observation", "DocumentReference", "Binary"];
