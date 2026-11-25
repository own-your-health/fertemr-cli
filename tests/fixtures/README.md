# Test fixtures

Every file here is **synthetic**. Field names, types, codes and statuses
follow the portal; everything else is invented, and no value was copied from
an answer.

`portal/` holds the portal API's answers, one per answer shape, as the stub
portal in `tests/common/mod.rs` serves them. The keys and nesting follow the
portal, with `null` where it sends `null`, and dates in its own formats
(ISO 8601 with an offset, and fractions of 2, 3, 6 or 7 digits; `M/DD/YYYY`
in the `Display…` fields). `tests/fixture_shapes.rs` pins the field names.
Which items there are, how many, in what order, which statuses occur
together, and every date, amount and text are invented for the tests.

| File | The answer to |
|---|---|
| `config.json` | `GET /config.json` on the portal's web host; `{BASE}` is the stub's API base |
| `login-ok.json` | `User/LoginAttempt`, signed in |
| `user.json` | `User/GetUserPrefs`: the envelope (`c` 0), `d` the signed-in user with the session id |
| `login-refused.json` | `User/LoginAttempt` with a wrong password: HTTP 500 and an envelope |
| `session-gone.json` | any call with a session the portal does not hold: HTTP 500 and an envelope |
| `not-found.json` | a list the clinic does not offer the patient: HTTP 500 and an envelope |
| `visits.json` | `PPVisits/List`: a cancelled visit, a past one and an upcoming one |
| `labs.json` | `PPLab/List`: two orders, one with a report PDF |
| `embryology.json` | `PPEmbryology/List`: one set of records |
| `files.json` | `PPFiles/List`: a file the clinic shared, and an external link |
| `messages-inbox.json` | `PPMessaging/List` for the Inbox: a message and its reply, one with two attachments, an older one, a draft and a trashed one |
| `messages-empty.json` | `PPMessaging/List` for a folder with no messages |
| `hp-instances.json` | `PPHP/ListInstances`: the intake forms assigned |
| `hp-instance.json` | `PPHP/Instance`: one form's sections, questions, options and answers |
| `calendar.json`, `calendar-empty.json` | `PPCalendar/List`, with and without events |

How a form answer names one of its question's options is not yet
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
