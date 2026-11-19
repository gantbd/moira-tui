use moira_client::DeliberationReadOnlyClient;
use moira_protocol::deliberation as d;
use moira_tui::{DeliberationObservation, observe_deliberation};
use moira_tui_local_client::LocalDeliberationPort;

pub fn run(
    port: LocalDeliberationPort,
    id: d::DeliberationIdV1,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = DeliberationReadOnlyClient::new(port);
    let value = match observe_deliberation(&mut client, id) {
        DeliberationObservation::Current {
            registry_revision,
            view,
            review,
        } => serde_json::json!({
            "version": 1,
            "capability": "deliberation.inspect.v1",
            "status": "current",
            "registry_revision": registry_revision,
            "deliberation": moira_tui::deliberation_view_json(*view),
            "review": review.map(|value| moira_tui::review_view_json(*value)),
        }),
        DeliberationObservation::NotFound { registry_revision } => serde_json::json!({
            "version": 1,
            "capability": "deliberation.inspect.v1",
            "status": "not_found",
            "registry_revision": registry_revision,
        }),
        DeliberationObservation::Stale => envelope("stale"),
        DeliberationObservation::NeedsReopen => envelope("needs_reopen"),
        DeliberationObservation::Refused(refusal) => serde_json::json!({
            "version": 1,
            "capability": "deliberation.inspect.v1",
            "status": "refused",
            "refusal": refusal,
        }),
        DeliberationObservation::InvalidReply => envelope("invalid_reply"),
        DeliberationObservation::Transport => envelope("transport_unavailable"),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn envelope(status: &str) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "capability": "deliberation.inspect.v1",
        "status": status,
    })
}
