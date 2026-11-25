# The FERT patient portal API

FERT is a fertility EMR by Redshift Technologies. A clinic on it has a patient
portal at `https://portal-<clinic>.fertemr.com`: a Flutter web app that
calls an ASP.NET API on another host of the clinic's (commonly
`portal-<clinic>-api.fertemr.com`). fertemr calls that API the way the app
does. Everything below was read from the app's code and its traffic, except
where it says otherwise.

## Route

| Question | Answer |
| --- | --- |
| A patient FHIR API (ONC certified, SMART on FHIR) | None. ONC's Certified Health IT Product List (chpl.healthit.gov) lists no product by Redshift Technologies or named FERT; the one match for "FERT" is an unrelated vendor's. On the API host, `/fhir/metadata`, `/fhir/r4/metadata` and `/.well-known/smart-configuration` answer 404, and so do `/metadata` and `/swagger` |
| A developer program | None for patients. The vendor's site advertises an "Open API Ecosystem" to clinics and their partners (scheduling, billing, labs, patient portals), with no public documentation, sandbox or sign-up |
| The portal's terms on automated access | The portal publishes none. Its sign-in answers carry the terms it wants accepted (`LatestTOSText`, `LatestTOSVersion`) |
| The route | The portal's own API, signed in as the patient, read-only |

So there is no API to ask for and no terms to weigh. fertemr signs in as
the patient and reads what the portal shows them: one request at a time,
each list read once, with no crawling beyond the patient's own records.

## Discovery

`GET https://portal-<clinic>.fertemr.com/config.json`, public, no
credential:

| Field | Meaning |
| --- | --- |
| `baseUrl` | the API base; fertemr calls `<baseUrl>/Portal/...` |
| `clientCode` | the clinic's code |
| `clientNameFull`, `clientNameShort` | the clinic's name |
| `environment` | `production` |

fertemr signs in only to a `baseUrl` on a `fertemr.com` host over https.
A portal with no `config.json` (404) is not a FERT portal (exit 4).

## Calls

Every call is `POST <base>/Portal/<Area>/<Call>` (the user and sign-out
calls are `GET`), with these headers:

| Header | Value |
| --- | --- |
| `content-type` | `application/json; charset=UTF-8` |
| `duuid` | a device id: a random UUID the app makes once per browser and keeps (`flutter.appUUID` in local storage) |
| `suuid` | the session id the sign-in answered (`flutter.sessUUID`); empty before |
| `co` | `1901-01-01T00:00:00Z`, always |

Every answer but the sign-in is an envelope `{"c", "t", "d", "r"}`: `c` 0 is
success and `d` the data; otherwise `t` is the portal's message. The sign-in
answers its fields at the top level, and an envelope when it refuses.

### Sign-in and session

| Call | Body | Answer |
| --- | --- | --- |
| `User/LoginAttempt` | `{"username", "password", "osDetail", "pushToken": ""}` | `success`, `uniqueSessionID` (the session), `ppUserId`, `ptId` (the patient), `FullName`, `FirstName`, `LastName`, `AcceptedLatestTOS`, the patient's notification settings |
| `GET User/GetUserPrefs` | none | the envelope, `d` the signed-in user with the sign-in answer's fields; a pure read, which `auth status --check` and the browser sign-in use |
| `GET User/Logout` | none | ends the session |

### Records

A list's body is `{"s": ""}`, as the app sends it. Each list comes whole:
the app sends no date range or page, and neither does fertemr.

| Call | Body | What `d` holds |
| --- | --- | --- |
| `PPVisits/List` | `{"s": ""}` | `Results[]`: `ID`, `VisitDate`, `Type`, `Status`, `ApptDuration` and its units, `Facility` and its address |
| `PPLab/List` | `{"s": ""}` | `Results[]`: one row per result, with its order (`OrderID`), test, value, unit, range, flag, status, specimen date, and the order's report (`ResultPDFID`, `RecordTypeID`) |
| `PPLab/GetByID` | `{"ID", "RecordTypeID"}` | one result |
| `PPEmbryology/List` | `{"s": ""}` | `Set` (which columns the clinic shows, the publish date) and `Records[]`: `RecordID`, the sources, device and embryo numbers, `Stage`, `Grade`, PGT results, `FreezeDate`, `XferDate`, `DiscardDate` |
| `PPFiles/List` | `{"s": ""}` | `Results[]`: files the clinic shared, `IsExternal` for a link rather than a file |
| `PPMessaging/List` | `{"Folder", "DtFrom", "DtTo", "UnreadOnly", "NotReplied", "SearchTerm", "TotalMessages", "TotalPages", "Page", "PageSize", "Messages": [], "IsReadArchTrash", "Options_PageSize", "Selected_PageSize"}`, `Folder` one of `Inbox`, `Sent`, `Archive`, `Draft`, dates `1901-01-01T00:00:00.000` | the same object with `Messages[]` filled: each with its thread (`MessagesInThread`), `Attachments[]` (`ID`, `FileName`) and a related lab result |
| `PPHP/ListInstances` | `{"s": ""}` | the intake forms assigned (`Instances[]`) and their templates |
| `PPHP/Instance` | `{"id"}` | one form: its template, `Sections`, `Questions`, `QuestionAnswerOptions` and `Answers` |
| `PPCalendar/List` | `{"s": ""}` | the treatment calendar: cycles, events, templates |
| `PPDashboard/InitialData` | `{"s": ""}` | the dashboard's counts and the assigned forms; fertemr does not read it |

### Downloads

Each answers the file's bytes with HTTP 200, not an envelope.

| Call | Body |
| --- | --- |
| `PPFiles/DownloadFile` | `{"FileID"}` |
| `PPLab/DownloadFile` | `{"FileID", "RecordTypeID"}`: a lab report PDF |
| `PPMessaging/DownloadAttachment` | `{"FileID"}` |
| `PPVisits/DownloadCalEvent` | `{"visitid"}`: a visit as a calendar file; fertemr does not read it |

### Writes, which fertemr never sends

`PPMessaging/SetReadStatus` (the app sends it when a message is opened),
`PPMessaging/SaveOrSendMessage`, `User/SaveUserPrefs`, `User/AcceptTOS`, and
the form saves. fertemr sends only paths whose last segment is one of the
reads above (`portal::READS`), and `fertemr api` refuses anything else before
a request is made, downloads included (`documents download` fetches and
checks those).

## Traps

- **Status codes say little.** A refused sign-in, a session the portal does
  not hold and a list the clinic does not offer all answer HTTP 500, with an
  envelope whose `t` tells them apart. fertemr reads `t`: "The user name or
  password provided is incorrect." is a refused sign-in (exit 3), "Existing
  session not found, please log in." an ended session (exit 3), and "The
  requested resource was not found" a list the patient does not have (a
  skip in the export, exit 4 from `api`).
- **The current user comes in the envelope.** Unlike the sign-in answer it
  repeats, `User/GetUserPrefs` answers `{"c": 0, "t": "", "d": <the user>,
  "r": null}`. Read as a bare sign-in answer, a live session looks like an
  error.
- **The session is bound to its device id.** The right `suuid` with another
  `duuid` is "Existing session not found", and the session stays good with
  the right pair. fertemr keeps the device id with the session and reuses it
  for the profile's later sign-ins.
- **Opening a message in the app marks it read** (`SetReadStatus`). fertemr
  reads the lists only, so messages it exports stay unread in the portal.
- **Drafts are the patient's own unsent text.** fertemr leaves the `Draft`
  folder and trashed messages out.
- **A download can be an error page.** fertemr checks a file against what its
  name says it is (a PDF starts `%PDF`) before writing it, and skips one that
  is not (`verify_failed`).
- **A form answer's option.** A choice question's answer names one of its
  `QuestionAnswerOptions`. Whether it names the option's `ID` or its
  `Value` is not confirmed; fertemr reads either, and a comma-separated list
  of either.
- **Dates come in several spellings**: ISO 8601 with and without an offset,
  fractions of 2 to 7 digits, `M/D/YYYY` in the `Display…` fields, and
  `1901-01-01` for "none". fertemr keeps a time only with its zone, and
  treats a year before 1902 as no date.
- **Sessions are short.** The portal ends a session after somewhere between
  20 and 40 minutes idle ("Existing session not found"), and may also end one
  at a fixed age; the exact rule is not confirmed. fertemr keeps no password,
  so a pull usually starts with a sign-in. When the portal has ended a
  session, every command is exit 3 naming it.
- **Terms.** A sign-in answering `AcceptedLatestTOS: false` means the portal
  wants new terms accepted in its app. fertemr says so and never accepts them.
