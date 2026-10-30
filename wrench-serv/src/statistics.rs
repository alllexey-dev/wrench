//! Usage analytics sent to PostHog, and the tracking cookie.

use serde_json::{json, Value};
use uuid::Uuid;

// TODO: make it configurable
const POSTHOG_API_KEY: &str = "phc_4MVBHknwF8Qok57n2J5S9OVP3z6BpRJM4fiDtH7rGg7";

pub enum Event {
    GetForm {
        posthog_id: String,
    },
    Simulation {
        guid: String,
        name: String,
        isa: String,
        variant: Option<String>,
        asm_sha1: String,
        yaml_sha1: String,
        win_count: usize,
        fail_count: usize,
        posthog_id: String,
        success: bool,
        duration: u64,
        variant_success: Option<bool>,
    },
    ReportView {
        guid: String,
        name: String,
        posthog_id: String,
        wrench_version: String,
    },
}

fn payload(event: Event) -> Value {
    let version = wrench::version();
    let (name, id, properties) = match event {
        Event::GetForm { posthog_id } => ("wrench:get_form", posthog_id, json!({ "wrench_version": version })),
        Event::Simulation {
            guid,
            name,
            isa,
            variant,
            asm_sha1,
            yaml_sha1,
            win_count,
            fail_count,
            posthog_id,
            success,
            duration,
            variant_success,
        } => (
            "wrench:simulation_submit",
            posthog_id,
            json!({
                "authorName": name,
                "simulation_guid": guid,
                "wrench_version": version,
                "isa": isa,
                "variant": variant,
                "asm_sha1": asm_sha1,
                "yaml_sha1": yaml_sha1,
                "win_count": win_count,
                "fail_count": fail_count,
                "success": success,
                "duration": duration,
                "variant_success": variant_success,
                "$set": { "name": name },
            }),
        ),
        Event::ReportView {
            guid,
            name,
            posthog_id,
            wrench_version,
        } => (
            "wrench:report_view",
            posthog_id,
            json!({
                "authorName": name,
                "simulation_guid": guid,
                "version": version,
                "wrench_version": wrench_version,
            }),
        ),
    };
    json!({ "api_key": POSTHOG_API_KEY, "event": name, "distinct_id": id, "properties": properties })
}

/// Send in the background; a failure is only logged.
pub fn track_event(event: Event) {
    let body = payload(event);
    std::thread::spawn(move || {
        if let Err(e) = ureq::post("https://eu.i.posthog.com/i/v0/e").send_json(body) {
            println!("Posthog tracking error: {}", e);
        }
    });
}

fn cookies(header: Option<&str>) -> Vec<(String, String)> {
    header
        .unwrap_or("")
        .split(';')
        .filter_map(|kv| {
            let kv = kv.trim();
            if kv.is_empty() {
                return None;
            }
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// The `track_id` cookie, or a new id.
pub fn get_track(cookie: Option<&str>) -> String {
    let found: Vec<String> = cookies(cookie)
        .into_iter()
        .filter(|(k, _)| k == "track_id")
        .map(|(_, v)| v)
        .collect();
    match found.as_slice() {
        [v] => v.clone(),
        _ => Uuid::new_v4().to_string(),
    }
}

pub fn track_cookie(track: &str) -> String {
    format!("track_id={}; path=/; Max-Age=10368000", track)
}

/// The PostHog `distinct_id` from its cookie, or `fallback`.
pub fn get_posthog_id_from_cookie(cookie: Option<&str>, fallback: &str) -> String {
    let name = format!("ph_{}_posthog", POSTHOG_API_KEY);
    let found: Vec<String> = cookies(cookie)
        .into_iter()
        .filter(|(k, _)| *k == name)
        .map(|(_, v)| v)
        .collect();
    let [raw] = found.as_slice() else {
        return fallback.to_string();
    };
    let decoded = percent_decode(raw);
    serde_json::from_slice::<Value>(&decoded)
        .ok()
        .and_then(|v| v.get("distinct_id").and_then(Value::as_str).map(String::from))
        .unwrap_or_else(|| fallback.to_string())
}

/// `%XX` escapes decoded; everything else kept.
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

pub fn post_hog_tracker() -> String {
    [
        "<script>",
        "    !function(t,e){var o,n,p,r;e.__SV||(window.posthog=e,e._i=[],e.init=function(i,s,a){function g(t,e){var o=e.split(\".\");2==o.length&&(t=t[o[0]],e=o[1]),t[e]=function(){t.push([e].concat(Array.prototype.slice.call(arguments,0)))}}(p=t.createElement(\"script\")).type=\"text/javascript\",p.crossOrigin=\"anonymous\",p.async=!0,p.src=s.api_host.replace(\".i.posthog.com\",\"-assets.i.posthog.com\")+\"/static/array.js\",(r=t.getElementsByTagName(\"script\")[0]).parentNode.insertBefore(p,r);var u=e;for(void 0!==a?u=e[a]=[]:a=\"posthog\",u.people=u.people||[],u.toString=function(t){var e=\"posthog\";return\"posthog\"!==a&&(e+=\".\"+a),t||(e+=\" (stub)\"),e},u.people.toString=function(){return u.toString(1)+\".people (stub)\"},o=\"init capture register register_once register_for_session unregister unregister_for_session getFeatureFlag getFeatureFlagPayload isFeatureEnabled reloadFeatureFlags updateEarlyAccessFeatureEnrollment getEarlyAccessFeatures on onFeatureFlags onSurveysLoaded onSessionId getSurveys getActiveMatchingSurveys renderSurvey canRenderSurvey canRenderSurveyAsync identify setPersonProperties group resetGroups setPersonPropertiesForFlags resetPersonPropertiesForFlags setGroupPropertiesForFlags resetGroupPropertiesForFlags reset get_distinct_id getGroups get_session_id get_session_replay_url alias set_config startSessionRecording stopSessionRecording sessionRecordingStarted captureException loadToolbar get_property getSessionProperty createPersonProfile opt_in_capturing opt_out_capturing has_opted_in_capturing has_opted_out_capturing clear_opt_in_out_capturing debug getPageViewId captureTraceFeedback captureTraceMetric\".split(\" \"),n=0;n<o.length;n++)g(u,o[n]);e._i.push([i,s,a])},e.__SV=1)}(document,window.posthog||[]);",
        &format!("    posthog.init('{}', {{", POSTHOG_API_KEY),
        "        api_host: 'https://eu.i.posthog.com',",
        "        person_profiles: 'identified_only', // or 'always' to create profiles for anonymous users as well",
        "    })",
        "</script>",
    ]
    .iter()
    .map(|l| format!("{}\n", l))
    .collect()
}
