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
        } => serde_json::json!({
            "version": 1,
            "capability": "deliberation.inspect.v1",
            "status": "current",
            "registry_revision": registry_revision,
            "deliberation": view_json(*view),
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

fn view_json(value: d::DeliberationViewV1) -> serde_json::Value {
    serde_json::json!({
        "protocol_version": value.version,
        "phase": phase(value.phase),
        "remaining_fuel": value.remaining_fuel,
        "question_digest": hex(&value.plan.question_digest),
        "admission": {
            "fitness": fitness(value.plan.admission.fitness),
            "affected_population_digest": hex(&value.plan.admission.affected_population_digest),
            "stakeholder_map_digest": hex(&value.plan.admission.stakeholder_map_digest),
            "coverage_manifest_digest": hex(&value.plan.admission.coverage_manifest_digest),
            "equity_assessment_digest": hex(&value.plan.admission.equity_assessment_digest),
            "assessor_principal": hex(&value.plan.admission.assessor_principal),
        },
        "method": {
            "machine_digest": hex(&value.plan.method.machine_digest),
            "state_digest": hex(&value.plan.method.state_digest),
        },
        "claims": value.plan.claims.into_iter().map(claim).collect::<Vec<_>>(),
        "criteria": value.plan.criteria.into_iter().map(|value| serde_json::json!({
            "id": hex(&value.id.0),
            "direction": match value.direction {
                d::CriterionDirectionV1::Maximize => "maximize",
                d::CriterionDirectionV1::Minimize => "minimize",
            },
        })).collect::<Vec<_>>(),
        "candidates": value.plan.candidates.into_iter().map(|value| serde_json::json!({
            "id": hex(&value.id.0),
            "scores": value.scores,
        })).collect::<Vec<_>>(),
        "members": value.plan.members.into_iter().map(|value| serde_json::json!({
            "id": hex(&value.id.0),
            "persona": persona(value.lens),
            "stakeholder": {
                "principal": hex(&value.stakeholder.principal),
                "constituency_digest": hex(&value.stakeholder.constituency_digest),
                "representation": representation(value.stakeholder.representation),
                "accountable_principal": value.stakeholder.accountable_principal.map(|value| hex(&value)),
                "mandate_digest": value.stakeholder.mandate_digest.map(|value| hex(&value)),
                "conflict_digest": value.stakeholder.conflict_digest.map(|value| hex(&value)),
            },
            "concern_root_digest": hex(&value.concern_root_digest),
            "grounding": {
                "authority_digest": hex(&value.grounding.authority_digest),
                "surface_digest": hex(&value.grounding.surface_digest),
                "depth": depth(value.grounding.depth),
            },
        })).collect::<Vec<_>>(),
        "orthogonality": {
            "query_digest": hex(&value.plan.orthogonality.query_digest),
            "result_digest": hex(&value.plan.orthogonality.result_digest),
            "all_orthogonal": value.plan.orthogonality.all_orthogonal,
        },
        "fuel": value.plan.fuel,
        "reports": value.reports.into_iter().map(|value| serde_json::json!({
            "id": hex(&value.id.0),
            "member": hex(&value.member.0),
            "output_digest": hex(&value.output_digest),
            "evidence_digests": value.evidence_digests.into_iter().map(|value| hex(&value)).collect::<Vec<_>>(),
            "dissent_digests": value.dissent_digests.into_iter().map(|value| hex(&value)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "resolution": value.resolution.map(resolution_json),
        "limits": {
            "max_claims": value.limits.max_claims,
            "max_criteria": value.limits.max_criteria,
            "max_candidates": value.limits.max_candidates,
            "max_members": value.limits.max_members,
            "max_evidence_per_report": value.limits.max_evidence_per_report,
            "max_dissent_per_report": value.limits.max_dissent_per_report,
            "max_fuel": value.limits.max_fuel,
        },
    })
}

fn resolution_json(value: d::ResolutionV1) -> serde_json::Value {
    match value {
        d::ResolutionV1::ProvenBool {
            value,
            validator,
            evidence_digest,
        } => serde_json::json!({
            "kind": "proven_bool",
            "value": value,
            "validator": hex(&validator),
            "evidence_digest": hex(&evidence_digest),
        }),
        d::ResolutionV1::AuthorizedChoice {
            frontier,
            selected,
            authority,
        } => serde_json::json!({
            "kind": "authorized_choice",
            "frontier": frontier.into_iter().map(|value| hex(&value.0)).collect::<Vec<_>>(),
            "selected": hex(&selected.0),
            "authority": {
                "decision": hex(&authority.decision.0),
                "subject": hex(&authority.subject.0),
                "epoch": authority.epoch,
            },
        }),
        d::ResolutionV1::Maybe(value) => serde_json::json!({
            "kind": "maybe",
            "value": match value {
                d::MaybeValueV1::TrueOnly => "true_only",
                d::MaybeValueV1::FalseOnly => "false_only",
                d::MaybeValueV1::Both => "both",
                d::MaybeValueV1::Neither => "neither",
            },
        }),
        d::ResolutionV1::Mu(value) => serde_json::json!({
            "kind": "mu",
            "reason": match value {
                d::MuReasonV1::FuelExhausted => "fuel_exhausted",
                d::MuReasonV1::MissingEvidence => "missing_evidence",
                d::MuReasonV1::ContradictoryEvidence => "contradictory_evidence",
                d::MuReasonV1::WrongQuestion => "wrong_question",
                d::MuReasonV1::WrongEnsemble => "wrong_ensemble",
                d::MuReasonV1::NoAuthorizedSelector => "no_authorized_selector",
            },
        }),
    }
}

fn claim(value: d::ClaimKindV1) -> &'static str {
    match value {
        d::ClaimKindV1::Formal => "formal",
        d::ClaimKindV1::Empirical => "empirical",
        d::ClaimKindV1::Normative => "normative",
        d::ClaimKindV1::Historical => "historical",
        d::ClaimKindV1::Creative => "creative",
    }
}

fn persona(value: d::PersonaLensV1) -> &'static str {
    match value {
        d::PersonaLensV1::Historian => "historian",
        d::PersonaLensV1::IdeaStormer => "idea_stormer",
        d::PersonaLensV1::Theorist => "theorist",
        d::PersonaLensV1::Researcher => "researcher",
        d::PersonaLensV1::RigorReviewer => "rigor_reviewer",
        d::PersonaLensV1::ConvergenceFacilitator => "convergence_facilitator",
    }
}

fn fitness(value: d::EnsembleFitnessV1) -> &'static str {
    match value {
        d::EnsembleFitnessV1::Yes => "yes",
        d::EnsembleFitnessV1::No => "no",
        d::EnsembleFitnessV1::Contested => "contested",
    }
}

fn representation(value: d::StakeholderRepresentationV1) -> &'static str {
    match value {
        d::StakeholderRepresentationV1::ActualStakeholder => "actual_stakeholder",
        d::StakeholderRepresentationV1::AuthorizedRepresentative => "authorized_representative",
        d::StakeholderRepresentationV1::AgentModeledPerspective => "agent_modeled_perspective",
        d::StakeholderRepresentationV1::TechnicalSme => "technical_sme",
    }
}

fn depth(value: d::SurfaceDepthV1) -> &'static str {
    match value {
        d::SurfaceDepthV1::Ambient => "ambient",
        d::SurfaceDepthV1::Activated => "activated",
        d::SurfaceDepthV1::Reference => "reference",
    }
}

fn phase(value: d::DeliberationPhaseV1) -> &'static str {
    match value {
        d::DeliberationPhaseV1::Collecting => "collecting",
        d::DeliberationPhaseV1::ReadyToResolve => "ready_to_resolve",
        d::DeliberationPhaseV1::Resolved => "resolved",
    }
}

fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(value.len() * 2);
    for byte in value {
        result.push(char::from(DIGITS[usize::from(byte >> 4)]));
        result.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    result
}
