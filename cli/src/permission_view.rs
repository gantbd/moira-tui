//! Stable, read-only projection of one governed provider permission request.

use std::{error::Error, io};

use moira_client::{AgentParticipationReadOnlyClient, AgentReadOnlyClient};
use moira_protocol::{agent, agent_participation as participation};
use moira_tui_local_client::{LocalAgentParticipationPort, LocalAgentPort};
use serde_json::{Value, json};
#[cfg(test)]
use sha2::{Digest, Sha256};

const PAGE_LIMIT: u64 = 64;
const MAX_PAGES: usize = 128;

pub fn run(
    participation_port: LocalAgentParticipationPort,
    agent_port: LocalAgentPort,
    id: participation::ParticipationIdV1,
) -> Result<(), Box<dyn Error>> {
    let mut participations = AgentParticipationReadOnlyClient::new(participation_port);
    let first = match participations.participation(id).map_err(transport)? {
        participation::ReplyV1::Accepted(value) => match *value {
            participation::OutcomeV1::Participation(Some(view)) => view,
            participation::OutcomeV1::Participation(None) => {
                return emit(json!({
                    "version": 1,
                    "capability": "agent.permission.confirmation-view.v1",
                    "status": "not_found"
                }));
            }
            _ => return emit_status("invalid_reply"),
        },
        participation::ReplyV1::Refused(refusal) => {
            return emit(json!({
                "version": 1,
                "capability": "agent.permission.confirmation-view.v1",
                "status": "refused",
                "refusal": format!("{refusal:?}")
            }));
        }
    };
    if first.phase != participation::PhaseV1::AwaitingPermission || first.permission.is_none() {
        return emit_status("not_awaiting_permission");
    }
    let confirmation = match participations
        .permission_confirmation(id)
        .map_err(transport)?
    {
        participation::ReplyV1::Accepted(value) => match *value {
            participation::OutcomeV1::PermissionConfirmation(Some(value)) => value,
            participation::OutcomeV1::PermissionConfirmation(None) => {
                return emit_status("not_awaiting_permission");
            }
            _ => return emit_status("invalid_reply"),
        },
        participation::ReplyV1::Refused(refusal) => {
            return emit(json!({
                "version": 1,
                "capability": "agent.permission.confirmation-view.v1",
                "status": "refused",
                "refusal": format!("{refusal:?}")
            }));
        }
    };
    if participation::permission_confirmation(confirmation.storage_revision, &first)
        != Some(confirmation)
    {
        return emit_status("inconsistent");
    }

    let session_id = agent::SessionIdV1(first.binding.session.0);
    let mut agents = AgentReadOnlyClient::new(agent_port);
    let before = match agents.inspect(session_id).map_err(transport)? {
        agent::ReplyV1::Accepted(agent::OutcomeV1::Session(view)) => view,
        agent::ReplyV1::Accepted(_) => return emit_status("invalid_reply"),
        agent::ReplyV1::Refused(refusal) => {
            return emit(json!({
                "version": 1,
                "capability": "agent.permission.confirmation-view.v1",
                "status": "refused",
                "refusal": format!("{refusal:?}")
            }));
        }
    };
    let records = match complete_events(&mut agents, session_id) {
        Ok(records) => records,
        Err(status) => return emit_status(status),
    };
    let after = match agents.inspect(session_id).map_err(transport)? {
        agent::ReplyV1::Accepted(agent::OutcomeV1::Session(view)) => view,
        agent::ReplyV1::Accepted(_) => return emit_status("invalid_reply"),
        agent::ReplyV1::Refused(_) => return emit_status("stale"),
    };
    let second = match participations.participation(id).map_err(transport)? {
        participation::ReplyV1::Accepted(value) => match *value {
            participation::OutcomeV1::Participation(Some(view)) => view,
            _ => return emit_status("stale"),
        },
        participation::ReplyV1::Refused(_) => return emit_status("stale"),
    };
    let second_confirmation = match participations
        .permission_confirmation(id)
        .map_err(transport)?
    {
        participation::ReplyV1::Accepted(value) => match *value {
            participation::OutcomeV1::PermissionConfirmation(Some(value)) => value,
            _ => return emit_status("stale"),
        },
        participation::ReplyV1::Refused(_) => return emit_status("stale"),
    };
    if first != second || before != after || confirmation != second_confirmation {
        return emit_status("stale");
    }
    let request = match validated_request(&first, &before, &records) {
        Some(request) => request,
        None => return emit_status("inconsistent"),
    };
    let evidence = first.permission.expect("validated present permission");
    emit(json!({
        "version": 1,
        "capability": "agent.permission.confirmation-view.v1",
        "status": "current",
        "participation": hex(&first.binding.participation.0),
        "session": hex(&first.binding.session.0),
        "revision": first.revision,
        "session_revision": before.revision,
        "attempt": {
            "task_key": hex(&first.binding.attempt.task_key),
            "incarnation": first.binding.attempt.incarnation,
            "attempt": first.binding.attempt.attempt,
        },
        "method": {
            "machine_digest": hex(&first.binding.method.machine_digest),
            "state_digest": hex(&first.binding.method.state_digest),
        },
        "grounding": {
            "authority_digest": hex(&first.binding.grounding.authority_digest),
            "surface_digest": hex(&first.binding.grounding.surface_digest),
            "depth": format!("{:?}", first.binding.grounding.depth),
        },
        "persona": format!("{:?}", first.binding.persona),
        "request": hex(&evidence.request.0),
        "turn": hex(&evidence.turn.0),
        "capability": request.capability,
        "capability_digest": hex(&evidence.capability_digest),
        "description": request.description,
        "options_digest": hex(&evidence.options_digest),
        "options": request.options.into_iter().map(|option| json!({
            "id": hex(&option.id.0),
            "kind": option_kind(option.kind),
            "label": option.label,
        })).collect::<Vec<_>>(),
        "storage_revision": confirmation.storage_revision,
        "confirmation_digest": hex(&confirmation.confirmation_digest),
    }))
}

fn complete_events(
    client: &mut AgentReadOnlyClient<LocalAgentPort>,
    session: agent::SessionIdV1,
) -> Result<Vec<agent::EventRecordV1>, &'static str> {
    let mut after = None;
    let mut previous = None;
    let mut records = Vec::new();
    for _ in 0..MAX_PAGES {
        let page = match client.events(session, after, PAGE_LIMIT) {
            Ok(agent::ReplyV1::Accepted(agent::OutcomeV1::Events(page))) => page,
            Ok(agent::ReplyV1::Accepted(_)) => return Err("invalid_reply"),
            Ok(agent::ReplyV1::Refused(_)) => return Err("refused"),
            Err(_) => return Err("outcome_unknown"),
        };
        if page.session != session {
            return Err("inconsistent");
        }
        for record in page.entries {
            let expected = previous.map_or(Some(0), |cursor: u64| cursor.checked_add(1));
            if Some(record.cursor) != expected {
                return Err("inconsistent");
            }
            previous = Some(record.cursor);
            records.push(record);
        }
        match page.next_after {
            Some(cursor) if Some(cursor) == previous => after = Some(cursor),
            Some(_) => return Err("inconsistent"),
            None => return Ok(records),
        }
    }
    Err("limit")
}

fn validated_request(
    participation: &participation::ParticipationViewV1,
    session: &agent::SessionViewV1,
    records: &[agent::EventRecordV1],
) -> Option<agent::PermissionRequestV1> {
    moira_tui::validated_permission_request(participation, session, records)
}

#[cfg(test)]
fn options_digest(values: &[agent::PermissionOptionV1]) -> [u8; 32] {
    moira_tui::permission_options_digest(values)
}

const fn option_kind(value: agent::PermissionOptionKindV1) -> &'static str {
    match value {
        agent::PermissionOptionKindV1::AllowOnce => "allow_once",
        agent::PermissionOptionKindV1::AllowAlways => "allow_always",
        agent::PermissionOptionKindV1::RejectOnce => "reject_once",
        agent::PermissionOptionKindV1::RejectAlways => "reject_always",
    }
}

fn emit_status(status: &str) -> Result<(), Box<dyn Error>> {
    emit(json!({
        "version": 1,
        "capability": "agent.permission.confirmation-view.v1",
        "status": status,
    }))
}

fn emit(value: Value) -> Result<(), Box<dyn Error>> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn transport(error: moira_tui_local_client::ClientError) -> io::Error {
    io::Error::other(format!("{error:?}"))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use moira_protocol::{WorkflowIdV1, deliberation};

    fn fixture() -> (
        participation::ParticipationViewV1,
        agent::SessionViewV1,
        Vec<agent::EventRecordV1>,
    ) {
        let options = vec![
            agent::PermissionOptionV1 {
                id: agent::PermissionOptionIdV1([21; 16]),
                kind: agent::PermissionOptionKindV1::AllowOnce,
                label: b"allow once".to_vec(),
            },
            agent::PermissionOptionV1 {
                id: agent::PermissionOptionIdV1([22; 16]),
                kind: agent::PermissionOptionKindV1::RejectOnce,
                label: b"reject".to_vec(),
            },
        ];
        let request = agent::PermissionRequestV1 {
            request: agent::PermissionRequestIdV1([19; 16]),
            turn: agent::TurnIdV1([18; 16]),
            capability: b"terminal: cargo test".to_vec(),
            description: b"run the bounded test suite".to_vec(),
            options: options.clone(),
        };
        let mut option_ids = [participation::PermissionOptionIdV1([0; 16]);
            participation::MAX_PERMISSION_OPTIONS_V1];
        option_ids[0] = participation::PermissionOptionIdV1(options[0].id.0);
        option_ids[1] = participation::PermissionOptionIdV1(options[1].id.0);
        let evidence = participation::PermissionEvidenceV1 {
            request: participation::PermissionRequestIdV1(request.request.0),
            turn: participation::AgentTurnIdV1(request.turn.0),
            capability_digest: Sha256::digest(&request.capability).into(),
            options_digest: options_digest(&options),
            option_count: 2,
            option_ids,
        };
        let view = participation::ParticipationViewV1 {
            binding: participation::BindingV1 {
                participation: participation::ParticipationIdV1([1; 16]),
                workflow: WorkflowIdV1([2; 16]),
                deliberation: deliberation::DeliberationIdV1([3; 16]),
                member: deliberation::EnsembleMemberIdV1([4; 16]),
                session: participation::AgentSessionIdV1([5; 16]),
                profile: participation::ProfileIdV1([6; 16]),
                attempt: participation::AttemptV1 {
                    task_key: [7; 16],
                    incarnation: 8,
                    attempt: 9,
                },
                method: deliberation::MethodRefV1 {
                    machine_digest: [10; 32],
                    state_digest: [11; 32],
                },
                grounding: deliberation::GroundingRefV1 {
                    authority_digest: [12; 32],
                    surface_digest: [13; 32],
                    depth: deliberation::SurfaceDepthV1::Activated,
                },
                persona: deliberation::PersonaLensV1::RigorReviewer,
            },
            revision: 14,
            phase: participation::PhaseV1::AwaitingPermission,
            session_revision: Some(15),
            last_event_cursor: Some(0),
            profile_document_digest: Some([16; 32]),
            capabilities: participation::CapabilitiesV1 {
                permissions: true,
                ..participation::CapabilitiesV1::default()
            },
            active_turn: Some(evidence.turn),
            permission: Some(evidence),
            terminal: None,
            pending: None,
        };
        let session = agent::SessionViewV1 {
            session: agent::SessionIdV1(view.binding.session.0),
            revision: 15,
            phase: agent::PhaseV1::AwaitingPermission,
            profile_document_digest: agent::DigestV1([16; 32]),
            active_turn: Some(request.turn),
            last_event_cursor: Some(0),
            capabilities: agent::CapabilitiesV1 {
                permissions: true,
                ..agent::CapabilitiesV1::default()
            },
            last_context: None,
        };
        let records = vec![agent::EventRecordV1 {
            cursor: 0,
            event: agent::EventV1::Permission(request),
        }];
        (view, session, records)
    }

    #[test]
    fn exact_governed_permission_event_is_presentable() {
        let (view, session, records) = fixture();
        let request = validated_request(&view, &session, &records).unwrap();
        assert_eq!(request.options.len(), 2);
        assert_ne!(
            participation::permission_confirmation(24, &view)
                .unwrap()
                .confirmation_digest,
            [0; 32]
        );
    }

    #[test]
    fn changed_option_content_or_duplicate_event_is_not_presentable() {
        let (view, session, mut records) = fixture();
        let agent::EventV1::Permission(request) = &mut records[0].event else {
            unreachable!()
        };
        request.options[0].label.push(b'!');
        assert!(validated_request(&view, &session, &records).is_none());

        let (_, session, mut records) = fixture();
        records.push(records[0].clone());
        assert!(validated_request(&view, &session, &records).is_none());
    }

    #[test]
    fn confirmation_fingerprint_binds_revision() {
        let (mut view, _, _) = fixture();
        let first = participation::permission_confirmation(24, &view)
            .unwrap()
            .confirmation_digest;
        view.revision += 1;
        assert_ne!(
            first,
            participation::permission_confirmation(24, &view)
                .unwrap()
                .confirmation_digest
        );
    }
}
