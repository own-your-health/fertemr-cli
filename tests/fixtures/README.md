# Test fixtures

Every file here is **synthetic**. No value in them is a real patient's, and no
real answer was copied to write them.

`portal/` holds the portal API's answers, one per answer shape, as the stub
portal in `tests/common/mod.rs` serves them. Their structure follows the
real answers: the same keys and nesting, the same types (ids are numbers,
durations strings, flags booleans), `null` where the portal sends `null`,
and dates in the portal's own formats (ISO 8601 with an offset, and
fractions of 2, 3, 6 or 7 digits; `M/DD/YYYY` in the `Display…` fields).
`tests/fixture_shapes.rs` pins the keys of every answer whose real shape has
been seen.

| File | The answer to |
|---|---|
| `config.json` | `GET /config.json` on the portal's web host; `{BASE}` is the stub's API base |
| `login-ok.json` | `User/LoginAttempt`, signed in |
| `user.json` | `User/GetUserPrefs`: the envelope (`c` 0), `d` the signed-in user with the session id |
| `login-refused.json` | `User/LoginAttempt` with a wrong password: HTTP 500 and an envelope |
| `session-gone.json` | any call with a session the portal does not hold: HTTP 500 and an envelope |
| `not-found.json` | a list the clinic does not offer the patient (embryology): HTTP 500 and an envelope |
| `visits.json` | `PPVisits/List`: one past visit, one upcoming |
| `labs.json` | `PPLab/List`: two orders, one with a report PDF |
| `embryology.json` | `PPEmbryology/List`: one set of records |
| `files.json` | `PPFiles/List`: a file the clinic shared, and an external link |
| `messages-inbox.json` | `PPMessaging/List` for the Inbox: a message with an attachment and a reply, an older one, and a draft |
| `messages-empty.json` | `PPMessaging/List` for a folder with no messages |
| `hp-instances.json` | `PPHP/ListInstances`: the intake forms assigned |
| `hp-instance.json` | `PPHP/Instance`: one form's sections, questions, options and answers |
| `calendar.json`, `calendar-empty.json` | `PPCalendar/List`, with and without events |

Lab results, files and embryology records have been seen only in the
portal's web app code, not in an answer: their keys are the ones the app
reads. How a form answer names one of its question's options is not yet
confirmed; `hp-instance.json` names it by the option's id, and fertemr also
reads it by the option's value.

Dummy values, enforced by `tests/no_personal_data.rs`:

| Field | Value |
|---|---|
| The patient | `Jane Example` (`Jane`, `Example`) |
| The clinic, its teams, labs and places | names with the word `Example`, or `Main` |
| Email | `@example.com` |
| Phone | `555-01xx` |
| Medical record number | zeros then one digit |
| Date of birth | a January first |
| Street address | `… Example St`, `Example City`, zip `00000` |
| Session id | `dummy-…` |
| Record ids | up to six digits |
