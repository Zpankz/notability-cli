//! "Coming up": events from connected Google / Microsoft calendars via short-lived OAuth tokens.

use reqwest::Method;
use serde_json::{Value, json};

use crate::config::{GOOGLE_CAL_SCOPE, MS_CAL_SCOPE};
use crate::error::{NbError, Result};
use crate::http::{Http, Req};

fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    )
}

pub fn normalize_google(e: &Value) -> Value {
    let pick = |k: &str| {
        e[k]["dateTime"]
            .as_str()
            .or_else(|| e[k]["date"].as_str())
            .map(Value::from)
            .unwrap_or(Value::Null)
    };
    json!({"provider": "GOOGLE", "id": e["id"], "title": e["summary"], "start": pick("start"), "end": pick("end"),
        "location": e["location"], "meeting_url": e["hangoutLink"], "status": e["status"]})
}

pub fn normalize_microsoft(e: &Value) -> Value {
    json!({"provider": "MICROSOFT", "id": e["id"], "title": e["subject"], "start": e["start"]["dateTime"],
        "end": e["end"]["dateTime"], "location": e["location"]["displayName"],
        "meeting_url": e["onlineMeeting"]["joinUrl"], "status": e["showAs"]})
}

impl Http {
    pub async fn calendar_events(
        &self,
        start_secs: i64,
        days: u32,
        max_results: u32,
    ) -> Result<Vec<Value>> {
        let accounts: Vec<Value> = self
            .connected_accounts()
            .await?
            .into_iter()
            .filter(|a| matches!(a["provider"].as_str(), Some("GOOGLE" | "MICROSOFT")))
            .collect();
        if accounts.is_empty() {
            return Err(NbError::not_found(
                "No calendar connected. Connect Google or Microsoft calendar in the Notability web app.",
            ));
        }
        let (t0, t1) = (
            rfc3339(start_secs),
            rfc3339(start_secs + i64::from(days) * 86_400),
        );
        let mut events = Vec::new();
        for acc in &accounts {
            let google = acc["provider"] == "GOOGLE";
            let tok = self
                .oauth_token(
                    acc,
                    if google {
                        GOOGLE_CAL_SCOPE
                    } else {
                        MS_CAL_SCOPE
                    },
                )
                .await?;
            let bearer = format!("Bearer {}", tok["accessToken"].as_str().unwrap_or_default());
            let req = if google {
                Req::new(
                    Method::GET,
                    "https://www.googleapis.com/calendar/v3/calendars/primary/events",
                )
                .query("timeMin", &t0)
                .query("timeMax", &t1)
                .query("singleEvents", "true")
                .query("orderBy", "startTime")
                .query("maxResults", max_results)
            } else {
                Req::new(
                    Method::GET,
                    "https://graph.microsoft.com/v1.0/me/calendarView",
                )
                .query("startDateTime", &t0)
                .query("endDateTime", &t1)
                .query("$top", max_results)
                .query("$orderby", "start/dateTime")
            };
            let body = self
                .send(req.no_auth().header("authorization", bearer))
                .await?
                .json()?;
            if google {
                events.extend(
                    body["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(normalize_google),
                );
            } else {
                events.extend(
                    body["value"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(normalize_microsoft),
                );
            }
        }
        events.sort_by(|a, b| {
            a["start"]
                .as_str()
                .unwrap_or_default()
                .cmp(b["start"].as_str().unwrap_or_default())
        });
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_and_normalisation() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_792_800_000), "2026-10-24T00:00:00Z");
        let g = normalize_google(
            &json!({"id": "1", "summary": "S", "start": {"date": "2026-10-27"}, "end": {"dateTime": "x"}}),
        );
        assert_eq!(g["start"], "2026-10-27");
        assert_eq!(g["end"], "x");
        let m = normalize_microsoft(
            &json!({"subject": "M", "location": {"displayName": "R"}, "showAs": "busy"}),
        );
        assert_eq!(
            (m["title"].as_str(), m["location"].as_str()),
            (Some("M"), Some("R"))
        );
    }
}
