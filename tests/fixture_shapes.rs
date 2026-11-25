//! The fixtures against the portal's field names, and fertemr's models
//! against the fixtures.
//!
//! The portal's field names are pinned here (names only: the values are
//! invented), so a fixture cannot drift from the portal. Then
//! each fixture is read through the model fertemr uses, and the fields the
//! mapping reads must come out set, so a renamed field fails here instead of
//! emptying a resource.

use std::collections::BTreeSet;

use fertemr::model::{
    Calendar, Embryology, FileItem, HealthProfile, LabResult, LoginAnswer, MessageList, PortalConfig, Results, Visit,
};
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/portal/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn d(name: &str) -> Value {
    let v = fixture(name);
    assert_eq!(v["c"], 0, "{name} is a successful envelope");
    v["d"].clone()
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn set(list: &str) -> BTreeSet<String> {
    list.split_whitespace().map(str::to_string).collect()
}

/// The field names the portal sends.
#[test]
fn fixtures_have_the_keys_the_portal_sends() {
    let login = fixture("login-ok.json");
    assert_eq!(
        keys(&login),
        set(
            "success err ppUserId fertUserId ptId uniqueSessionID Username FullName FirstName LastName \
             AllowEmail AllowSMS AllowPush EmailMsg SmsMsg PushMsg EmailCal SmsCal PushCal EmailFile SmsFile \
             PushFile EmailAppt SmsAppt PushAppt EmailResult SmsResult PushResult EmailForAlert PhoneForAlert \
             IsAppAdmin Timezone AcceptedLatestTOS LatestTOSText LatestTOSID LatestTOSVersion LatestTOSDate \
             TimezoneOptions"
        )
    );
    for name in ["login-refused.json", "session-gone.json", "not-found.json"] {
        assert_eq!(keys(&fixture(name)), set("c t d r"), "{name}");
    }
    let visit = &d("visits.json")["Results"][0];
    assert_eq!(
        keys(visit),
        set("ID VisitDate VisitDateStr Type ApptDuration ApptDurationUnits Status Facility Addr1 Addr2 City State Zip Country")
    );
    let hp = d("hp-instance.json");
    assert_eq!(
        keys(&hp),
        set(
            "InstanceID Instance Instances Answers SelectedAnswerOptions TemplateID SectionID Template Templates \
             Sections Questions QuestionAnswerOptions"
        )
    );
    assert_eq!(keys(&d("hp-instances.json")), keys(&hp));
    assert_eq!(
        keys(&hp["Instance"]),
        set(
            "ID HPTemplateID PatientID HPSubmissionStatusID HPReviewStatusID AssignedDt DisplayAssignedDt SubmitDt \
             DisplaySubmitDt ReviewedDt DisplayReviewedDt SourceHPInstanceID ReviewerNotes ModUserID ModEditDate Del"
        )
    );
    assert_eq!(
        keys(&hp["Answers"][0]),
        set(
            "ID HPInstanceID HPQuestionID PatientID AnswerText AnswerTextFull AnswerTextOther AnswerNum \
             MultiRecordRowID SourceHPAnswerID ReviewerNote ValidationErrors CrtDt ModUserID ModEditDate Del"
        )
    );
    assert_eq!(
        keys(&hp["Sections"][0]),
        set("ID HPTemplateID Name Description DisplayOrder ParentSectionID CrtDt ModUserID ModEditDate Del")
    );
    assert_eq!(
        keys(&hp["Questions"][0]),
        set(
            "ID HPSectionID PhreesiaID ParentQuestionID IsMultiRecord MaxRecords LabelL LabelLWidth LabelR \
             LabelRWidth LabelOther DisplayOrder IsRequired HPQuestionTypeID HPLayoutTypeID ExtraData AdminDesc \
             CrtDt Width ModUserID ModEditDate Del"
        )
    );
    assert_eq!(
        keys(&hp["QuestionAnswerOptions"][0]),
        set("ID HPQuestionID Text Value DisplayOrder CrtDt ModUserID ModEditDate Del")
    );
    assert_eq!(
        keys(&hp["Template"]),
        set("ID Name PhreesiaID Description IsPublished CrtDt DisplayCrtDt ModUserID ModEditDate Del")
    );
    let cal = d("calendar.json");
    assert_eq!(
        keys(&cal),
        set(
            "PtInPortal PtID KyCycleHdr CycleLinkID CalID LastUpdate HasCC CalCycles HasCM CalMasters HasCE \
             CalEvents CalEventsUp HasCT CalTemplates CurrentCal PtName PtMRN PtDOB"
        )
    );
    assert_eq!(keys(&d("calendar-empty.json")), keys(&cal));
    let inbox = d("messages-inbox.json");
    assert_eq!(
        keys(&inbox),
        set(
            "Folder DtFrom DtTo UnreadOnly NotReplied SearchTerm TotalMessages Page PageSize TotalPages \
             Options_PageSize Selected_PageSize Messages IsReadArchTrash"
        )
    );
    let m = &inbox["Messages"][0];
    for k in [
        "MsgID",
        "Subject",
        "Body",
        "PlainTextBody",
        "BodyIsHTML",
        "DtSentOrReceived",
        "FromGroupDisplay",
        "ToPatientDisplay",
        "Draft",
        "Trashed",
        "MessagesInThread",
        "Attachments",
    ] {
        assert!(m.get(k).is_some(), "a message carries {k}");
    }
}

#[test]
fn the_models_read_what_the_mapping_uses() {
    let cfg: PortalConfig = serde_json::from_value(fixture("config.json")).unwrap();
    assert!(cfg.base_url.is_some() && cfg.name_full.is_some());

    let login: LoginAnswer = serde_json::from_value(fixture("login-ok.json")).unwrap();
    assert_eq!(login.success, Some(true));
    assert!(login.session_id.is_some() && login.pt_id.is_some() && login.pp_user_id.is_some());
    assert!(login.full_name.is_some() && login.first_name.is_some() && login.last_name.is_some());
    let refused: LoginAnswer = serde_json::from_value(fixture("login-refused.json")).unwrap();
    assert!(refused.t.is_some() && refused.success.is_none());

    let visits: Results<Visit> = serde_json::from_value(d("visits.json")).unwrap();
    let v = &visits.results[0];
    assert!(v.id.is_some() && v.visit_date.is_some() && v.kind.is_some() && v.status.is_some());
    assert!(v.facility.is_some() && v.appt_duration.is_some());

    let labs: Results<LabResult> = serde_json::from_value(d("labs.json")).unwrap();
    let l = &labs.results[0];
    assert!(l.order_id.is_some() && l.lab_result_id.is_some() && l.test_name.is_some());
    assert!(l.result_value.is_some() && l.result_unit.is_some() && l.specimen_collection_date.is_some());
    assert!(l.result_pdf_id.is_some() && l.record_type_id.is_some() && l.lab_status_str.is_some());

    let e: Embryology = serde_json::from_value(d("embryology.json")).unwrap();
    assert!(e.set.publish_date_display.is_some());
    let r = &e.records[0];
    assert!(r.record_id.is_some() && r.stage.is_some() && r.grade.is_some() && r.freeze_date.is_some());

    let files: Results<FileItem> = serde_json::from_value(d("files.json")).unwrap();
    assert!(files
        .results
        .iter()
        .all(|f| f.id.is_some() && f.file_name.is_some() && f.is_external.is_some()));

    let msgs: MessageList = serde_json::from_value(d("messages-inbox.json")).unwrap();
    let m = msgs.messages.iter().find(|m| !m.messages_in_thread.is_empty()).unwrap();
    assert!(m.msg_id.is_some() && m.subject.is_some() && m.plain_text_body.is_some());
    assert!(m.dt_sent_or_received.is_some() && m.from_group_display.is_some());
    assert_eq!(m.messages_in_thread.len(), 1);
    assert!(msgs.messages.iter().any(|m| m.attachments.len() == 2));
    assert!(
        msgs.messages.iter().any(|m| m.draft == Some(true)) && msgs.messages.iter().any(|m| m.trashed == Some(true))
    );

    let hp: HealthProfile = serde_json::from_value(d("hp-instance.json")).unwrap();
    assert!(hp
        .instance
        .as_ref()
        .is_some_and(|i| i.id.is_some() && i.submit_dt.is_some()));
    assert!(!hp.answers.is_empty() && !hp.questions.is_empty() && !hp.sections.is_empty());
    assert!(hp.questions.iter().all(|q| q.section_id.is_some() && q.label.is_some()));
    assert!(hp
        .question_answer_options
        .iter()
        .all(|o| o.question_id.is_some() && o.text.is_some()));
    let list: HealthProfile = serde_json::from_value(d("hp-instances.json")).unwrap();
    assert!(!list.instances.is_empty() && !list.templates.is_empty());

    let cal: Calendar = serde_json::from_value(d("calendar.json")).unwrap();
    assert!(!cal.is_empty());
    let empty: Calendar = serde_json::from_value(d("calendar-empty.json")).unwrap();
    assert!(empty.is_empty());
}
