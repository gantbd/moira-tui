//! Stable read-only presentation of one methodical deliberation.

use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};

use moira_client::{DeliberationReadOnlyClient, DeliberationServicePort};
use moira_protocol::deliberation as d;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};

use crate::{Input, Lifecycle};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliberationObservation {
    Current {
        registry_revision: u64,
        view: Box<d::DeliberationViewV1>,
        review: Option<Box<d::ReviewViewV1>>,
    },
    NotFound {
        registry_revision: u64,
    },
    Stale,
    NeedsReopen,
    Refused(String),
    InvalidReply,
    Transport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HumanGateAgendaObservation {
    pub registry_revision: u64,
    pub deliberation: d::DeliberationIdV1,
    pub request: d::HumanGateRequestV1,
}

pub fn observe_deliberation<P: DeliberationServicePort>(
    client: &mut DeliberationReadOnlyClient<P>,
    id: d::DeliberationIdV1,
) -> DeliberationObservation {
    let first = match read_status(client.status()) {
        Ok(value) => value,
        Err(observation) => return observation,
    };
    if first.needs_reopen {
        return DeliberationObservation::NeedsReopen;
    }
    let view = match client.deliberation(id) {
        Ok(d::ReplyV1::Accepted(d::OutcomeV1::Deliberation(value))) => value,
        Ok(d::ReplyV1::Accepted(_)) => return DeliberationObservation::InvalidReply,
        Ok(d::ReplyV1::Refused(refusal)) => {
            return DeliberationObservation::Refused(format!("{refusal:?}"));
        }
        Err(_) => return DeliberationObservation::Transport,
    };
    let review = match client.review(id) {
        Ok(d::ReplyV1::Accepted(d::OutcomeV1::Review(value))) => value,
        Ok(d::ReplyV1::Accepted(_)) => return DeliberationObservation::InvalidReply,
        Ok(d::ReplyV1::Refused(refusal)) => {
            return DeliberationObservation::Refused(format!("{refusal:?}"));
        }
        Err(_) => return DeliberationObservation::Transport,
    };
    let second = match read_status(client.status()) {
        Ok(value) => value,
        Err(observation) => return observation,
    };
    if second.needs_reopen {
        DeliberationObservation::NeedsReopen
    } else if first.revision != second.revision {
        DeliberationObservation::Stale
    } else if let Some(view) = view {
        DeliberationObservation::Current {
            registry_revision: second.revision,
            view,
            review,
        }
    } else {
        DeliberationObservation::NotFound {
            registry_revision: second.revision,
        }
    }
}

fn read_status<E>(value: Result<d::ReplyV1, E>) -> Result<d::StatusV1, DeliberationObservation> {
    match value {
        Ok(d::ReplyV1::Accepted(d::OutcomeV1::Status(value))) => Ok(value),
        Ok(d::ReplyV1::Accepted(_)) => Err(DeliberationObservation::InvalidReply),
        Ok(d::ReplyV1::Refused(refusal)) => {
            Err(DeliberationObservation::Refused(format!("{refusal:?}")))
        }
        Err(_) => Err(DeliberationObservation::Transport),
    }
}

pub struct DeliberationComponent {
    requests: SyncSender<()>,
    replies: Receiver<DeliberationObservation>,
    pending: bool,
    observation: Option<(
        u64,
        Box<d::DeliberationViewV1>,
        Option<Box<d::ReviewViewV1>>,
    )>,
    selected: usize,
    notice: String,
}

impl DeliberationComponent {
    pub fn new<P>(port: P, id: d::DeliberationIdV1) -> Self
    where
        P: DeliberationServicePort + Send + 'static,
        P::Error: Send + 'static,
    {
        let (request_tx, request_rx) = sync_channel::<()>(1);
        let (reply_tx, reply_rx) = sync_channel::<DeliberationObservation>(1);
        std::thread::spawn(move || {
            let mut client = DeliberationReadOnlyClient::new(port);
            while request_rx.recv().is_ok() {
                if reply_tx
                    .send(observe_deliberation(&mut client, id))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut component = Self {
            requests: request_tx,
            replies: reply_rx,
            pending: false,
            observation: None,
            selected: 0,
            notice: "Methodical deliberation ready; refresh queued".into(),
        };
        component.offer();
        component
    }

    pub fn poll(&mut self) {
        loop {
            match self.replies.try_recv() {
                Ok(observation) => self.admit(observation),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.notice = "Moira deliberation worker unavailable".into();
                    break;
                }
            }
        }
    }

    /// Returns only an exact, stably observed pending gate. A settled response
    /// is not operator work, and absence is not inferred as approval.
    pub fn operator_agenda_observation(&self) -> Option<HumanGateAgendaObservation> {
        let (registry_revision, view, _) = self.observation.as_ref()?;
        if view.human_gate_response.is_some() {
            return None;
        }
        Some(HumanGateAgendaObservation {
            registry_revision: *registry_revision,
            deliberation: view.plan.id,
            request: view.human_gate_request?,
        })
    }

    pub fn update(&mut self, input: Input, focused: bool) -> Lifecycle {
        if !focused {
            return Lifecycle::Running;
        }
        match input {
            Input::Close => Lifecycle::CloseRequested,
            Input::Refresh => {
                self.offer();
                Lifecycle::Running
            }
            Input::Up | Input::Back => {
                self.selected = self.selected.saturating_sub(1);
                Lifecycle::Running
            }
            Input::Down | Input::Open => {
                let length = self.observation.as_ref().map_or(0, |(_, view, review)| {
                    detail_lines(view, review.as_deref(), 0).len()
                });
                if self.selected + 1 < length {
                    self.selected += 1;
                }
                Lifecycle::Running
            }
            Input::Toggle => Lifecycle::Running,
        }
    }

    pub fn draw_in(&self, frame: &mut Frame<'_>, area: Rect, focused: bool) {
        let [body, controls, notice] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let title = if focused {
            " Moira deliberation • focused "
        } else {
            " Moira deliberation "
        };
        let lines = self.observation.as_ref().map_or_else(
            || vec![Line::from("No stable deliberation observation")],
            |(_, view, review)| detail_lines(view, review.as_deref(), self.selected),
        );
        let visible = usize::from(body.height.saturating_sub(2)).max(1);
        let scroll = self.selected.saturating_add(1).saturating_sub(visible);
        frame.render_widget(
            Paragraph::new(lines)
                .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
                .block(Block::default().title(title).borders(Borders::ALL)),
            body,
        );
        frame.render_widget(
            Paragraph::new("j/k traverses exact facts • r refreshes • q closes"),
            controls,
        );
        frame.render_widget(Paragraph::new(self.notice.as_str()), notice);
    }

    fn offer(&mut self) {
        if self.pending {
            self.notice = "Moira deliberation request already pending".into();
        } else if self.requests.try_send(()).is_ok() {
            self.pending = true;
            self.notice = "Moira deliberation refresh pending".into();
        } else {
            self.notice = "Moira deliberation worker unavailable".into();
        }
    }

    fn admit(&mut self, observation: DeliberationObservation) {
        self.pending = false;
        match observation {
            DeliberationObservation::Current {
                registry_revision,
                view,
                review,
            } => {
                let length = detail_lines(&view, review.as_deref(), 0).len();
                self.selected = self.selected.min(length.saturating_sub(1));
                self.notice = format!(
                    "Stable registry revision {registry_revision}; no frontier or authority inferred"
                );
                self.observation = Some((registry_revision, view, review));
            }
            DeliberationObservation::NotFound { registry_revision } => {
                self.observation = None;
                self.notice = format!("No deliberation at registry revision {registry_revision}");
            }
            DeliberationObservation::Stale => {
                self.notice = "Deliberation changed during observation; refresh required".into();
            }
            DeliberationObservation::NeedsReopen => {
                self.notice = "Deliberation store requires reopen; state not inferred".into();
            }
            DeliberationObservation::Refused(refusal) => {
                self.notice = format!("Moira refused: {refusal}");
            }
            DeliberationObservation::InvalidReply => {
                self.notice = "Invalid Moira deliberation reply; state not inferred".into();
            }
            DeliberationObservation::Transport => {
                self.notice = "Moira unavailable; deliberation state not inferred".into();
            }
        }
    }
}

/// Exact owner-defined headless projection shared by standalone and composed
/// presentation hosts. It publishes evidence and standing, never authority.
pub fn deliberation_view_json(value: d::DeliberationViewV1) -> serde_json::Value {
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
        "human_gate": value.human_gate_request.map(|request| {
            human_gate_json(request, value.human_gate_response)
        }),
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

/// Exact retained review projection. Correlation is disclosed as observed,
/// missing, or shared; distinct values never become an independence claim.
pub fn review_view_json(value: d::ReviewViewV1) -> serde_json::Value {
    let summary = serde_json::json!({
        "reports": value.reports.len(),
        "findings": value.findings.len(),
        "rebuttals": value.rebuttals.len(),
        "dispositions": value.dispositions.len(),
    });
    serde_json::json!({
        "protocol_version": value.version,
        "deliberation": hex(&value.deliberation.0),
        "phase": review_phase(value.phase),
        "independence": independence(value.independence),
        "correlation": {
            "members": value.correlation.members,
            "dimensions": value.correlation.dimensions.into_iter().map(|evidence| serde_json::json!({
                "dimension": correlation_dimension(evidence.dimension),
                "status": if evidence.observed_members == 0 { "unknown" } else { "observed" },
                "observed_members": evidence.observed_members,
                "distinct_values": evidence.distinct_values,
                "shared": evidence.observed_members > 1 && evidence.distinct_values < evidence.observed_members,
            })).collect::<Vec<_>>(),
            "first_pass": {
                "committed_before_cross_review": value.correlation.first_pass.committed_before_cross_review,
                "exposed_before_commit": value.correlation.first_pass.exposed_before_commit,
                "unknown": value.correlation.first_pass.unknown,
            },
        },
        "summary": summary,
        "reports": value.reports.into_iter().map(|report| serde_json::json!({
            "id": hex(&report.id.0),
            "member": hex(&report.member.0),
            "output_digest": hex(&report.output_digest),
            "evidence_digests": report.evidence_digests.into_iter().map(|digest| hex(&digest)).collect::<Vec<_>>(),
            "dissent_digests": report.dissent_digests.into_iter().map(|digest| hex(&digest)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "findings": value.findings.into_iter().map(|finding| serde_json::json!({
            "id": hex(&finding.id.0),
            "source_report": hex(&finding.source_report.0),
            "owner": hex(&finding.owner),
            "finding_digest": hex(&finding.finding_digest),
            "evidence_digests": finding.evidence_digests.into_iter().map(|digest| hex(&digest)).collect::<Vec<_>>(),
            "requires_human_gate": finding.requires_human_gate,
        })).collect::<Vec<_>>(),
        "rebuttals": value.rebuttals.into_iter().map(|rebuttal| serde_json::json!({
            "id": hex(&rebuttal.id.0),
            "finding": hex(&rebuttal.finding.0),
            "member": hex(&rebuttal.member.0),
            "rebuttal_digest": hex(&rebuttal.rebuttal_digest),
            "evidence_digests": rebuttal.evidence_digests.into_iter().map(|digest| hex(&digest)).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "dispositions": value.dispositions.into_iter().map(|disposition| serde_json::json!({
            "id": hex(&disposition.id.0),
            "finding": hex(&disposition.finding.0),
            "owner": hex(&disposition.owner),
            "kind": finding_disposition(disposition.kind),
            "rationale_digest": hex(&disposition.rationale_digest),
            "evidence_digests": disposition.evidence_digests.into_iter().map(|digest| hex(&digest)).collect::<Vec<_>>(),
            "human_gate_response": disposition.human_gate_response.map(|id| hex(&id.0)),
            "decided_at": disposition.decided_at,
        })).collect::<Vec<_>>(),
        "human_gate_response": value.admitted_human_gate_response.map(|response| hex(&response.id.0)),
        "limits": {
            "max_findings": value.limits.max_findings,
            "max_rebuttals": value.limits.max_rebuttals,
            "max_evidence_per_item": value.limits.max_evidence_per_item,
        },
    })
}

fn human_gate_json(
    request: d::HumanGateRequestV1,
    response: Option<d::HumanGateResponseV1>,
) -> serde_json::Value {
    serde_json::json!({
        "status": if response.is_some() { "settled" } else { "pending" },
        "request": {
            "id": hex(&request.id.0),
            "reviewer": hex(&request.reviewer),
            "finding_digest": hex(&request.finding_digest),
            "decision_digest": hex(&request.decision_digest),
            "material_revision_digest": hex(&request.material_revision_digest),
            "policy_digest": hex(&request.policy_digest),
            "decision_packet": human_gate_material_json(request.decision_packet),
            "requested_at": request.requested_at,
        },
        "response": response.map(|response| serde_json::json!({
            "id": hex(&response.id.0),
            "request": hex(&response.request.0),
            "disposition": human_gate_disposition(response.disposition),
            "rationale_digest": hex(&response.rationale_digest),
            "rationale": human_gate_material_json(response.rationale),
            "responded_at": response.responded_at,
            "authority": {
                "decision": hex(&response.authority.decision),
                "subject": hex(&response.authority.subject),
                "epoch": response.authority.epoch,
            },
        })),
    })
}

fn human_gate_material_json(value: d::HumanGateMaterialRefV1) -> serde_json::Value {
    serde_json::json!({
        "owner_route": hex(&value.owner_route),
        "artifact_route": hex(&value.artifact_route),
        "expected_revision": value.expected_revision,
        "expected_sha256": hex(&value.expected_sha256),
    })
}

fn human_gate_disposition(value: d::HumanGateDispositionV1) -> &'static str {
    match value {
        d::HumanGateDispositionV1::Approved => "approved",
        d::HumanGateDispositionV1::Rejected => "rejected",
        d::HumanGateDispositionV1::Deferred => "deferred",
        d::HumanGateDispositionV1::ChangesRequested => "changes_requested",
    }
}

fn resolution_json(value: d::ResolutionV1) -> serde_json::Value {
    match value {
        d::ResolutionV1::ProvenBool {
            value,
            validator,
            evidence_digest,
        } => serde_json::json!({
            "kind": "proven_bool", "value": value, "validator": hex(&validator),
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
            "authority": { "decision": hex(&authority.decision.0), "subject": hex(&authority.subject.0), "epoch": authority.epoch },
        }),
        d::ResolutionV1::Maybe(value) => serde_json::json!({
            "kind": "maybe",
            "value": match value { d::MaybeValueV1::TrueOnly => "true_only", d::MaybeValueV1::FalseOnly => "false_only", d::MaybeValueV1::Both => "both", d::MaybeValueV1::Neither => "neither" },
        }),
        d::ResolutionV1::Mu(value) => serde_json::json!({
            "kind": "mu",
            "reason": match value { d::MuReasonV1::FuelExhausted => "fuel_exhausted", d::MuReasonV1::MissingEvidence => "missing_evidence", d::MuReasonV1::ContradictoryEvidence => "contradictory_evidence", d::MuReasonV1::WrongQuestion => "wrong_question", d::MuReasonV1::WrongEnsemble => "wrong_ensemble", d::MuReasonV1::NoAuthorizedSelector => "no_authorized_selector" },
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

fn detail_lines(
    view: &d::DeliberationViewV1,
    review: Option<&d::ReviewViewV1>,
    selected: usize,
) -> Vec<Line<'static>> {
    let mut values = vec![
        format!("phase: {:?}", view.phase),
        format!("remaining fuel: {}", view.remaining_fuel),
        format!("question: {}", digest(view.plan.question_digest)),
        format!(
            "right ensemble? {:?} assessor={}",
            view.plan.admission.fitness,
            identity(view.plan.admission.assessor_principal)
        ),
        format!(
            "ensemble coverage: affected={} stakeholder-map={} manifest={}",
            digest(view.plan.admission.affected_population_digest),
            digest(view.plan.admission.stakeholder_map_digest),
            digest(view.plan.admission.coverage_manifest_digest)
        ),
        format!(
            "ensemble equity: assessment={}",
            digest(view.plan.admission.equity_assessment_digest)
        ),
        format!(
            "method: machine={} state={}",
            digest(view.plan.method.machine_digest),
            digest(view.plan.method.state_digest)
        ),
        format!(
            "orthogonality: exact={} query={} result={}",
            view.plan.orthogonality.all_orthogonal,
            digest(view.plan.orthogonality.query_digest),
            digest(view.plan.orthogonality.result_digest)
        ),
        format!("claims: {:?}", view.plan.claims),
    ];
    for member in &view.plan.members {
        values.push(format!(
            "persona {:?}: member={} concern={} grounding={:?}/{}",
            member.lens,
            identity(member.id.0),
            digest(member.concern_root_digest),
            member.grounding.depth,
            digest(member.grounding.surface_digest)
        ));
        values.push(format!(
            "stakeholder {:?}: principal={} constituency={}",
            member.stakeholder.representation,
            identity(member.stakeholder.principal),
            digest(member.stakeholder.constituency_digest)
        ));
        values.push(format!(
            "stakeholder standing: accountable={} mandate={} conflict={}",
            optional_identity(member.stakeholder.accountable_principal),
            optional_digest(member.stakeholder.mandate_digest),
            optional_digest(member.stakeholder.conflict_digest)
        ));
    }
    for report in &view.reports {
        values.push(format!(
            "report {} member={} output={} evidence={} dissent={}",
            identity(report.id.0),
            identity(report.member.0),
            digest(report.output_digest),
            report.evidence_digests.len(),
            report.dissent_digests.len()
        ));
    }
    for candidate in &view.plan.candidates {
        values.push(format!(
            "candidate {} scores={:?}",
            identity(candidate.id.0),
            candidate.scores
        ));
    }
    values.push(resolution_summary(view.resolution.as_ref()));
    values.extend(human_gate_summary(
        view.human_gate_request.as_ref(),
        view.human_gate_response.as_ref(),
    ));
    values.extend(review_summary(review));
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            Line::from(format!(
                "{} {value}",
                if index == selected { ">" } else { " " }
            ))
        })
        .collect()
}

fn review_summary(review: Option<&d::ReviewViewV1>) -> Vec<String> {
    let Some(review) = review else {
        return vec!["review: not begun; correlation not yet retained".into()];
    };
    let mut values = vec![
        format!(
            "review: {:?} reports={} findings={} rebuttals={} dispositions={}",
            review.phase,
            review.reports.len(),
            review.findings.len(),
            review.rebuttals.len(),
            review.dispositions.len(),
        ),
        format!(
            "review correlation: {:?} members={}",
            review.independence, review.correlation.members
        ),
        format!(
            "first pass: committed-before-cross-review={} exposed-before-commit={} unknown={}",
            review.correlation.first_pass.committed_before_cross_review,
            review.correlation.first_pass.exposed_before_commit,
            review.correlation.first_pass.unknown,
        ),
    ];
    values.extend(review.correlation.dimensions.iter().map(|evidence| {
        format!(
            "correlation {}: {} observed={} distinct={} shared={}",
            correlation_dimension(evidence.dimension),
            if evidence.observed_members == 0 {
                "unknown"
            } else {
                "observed"
            },
            evidence.observed_members,
            evidence.distinct_values,
            evidence.observed_members > 1 && evidence.distinct_values < evidence.observed_members,
        )
    }));
    for finding in &review.findings {
        values.push(format!(
            "finding {} report={} owner={}",
            identity(finding.id.0),
            identity(finding.source_report.0),
            identity(finding.owner),
        ));
        values.push(format!(
            "finding evidence: digest={} evidence={} human-gate={}",
            digest(finding.finding_digest),
            finding.evidence_digests.len(),
            finding.requires_human_gate,
        ));
    }
    for rebuttal in &review.rebuttals {
        values.push(format!(
            "rebuttal {} finding={} member={}",
            identity(rebuttal.id.0),
            identity(rebuttal.finding.0),
            identity(rebuttal.member.0),
        ));
        values.push(format!(
            "rebuttal evidence: digest={} evidence={}",
            digest(rebuttal.rebuttal_digest),
            rebuttal.evidence_digests.len(),
        ));
    }
    for disposition in &review.dispositions {
        values.push(format!(
            "disposition {} finding={} owner={} kind={}",
            identity(disposition.id.0),
            identity(disposition.finding.0),
            identity(disposition.owner),
            finding_disposition(disposition.kind),
        ));
        values.push(format!(
            "disposition evidence: rationale={} evidence={} human-gate={} decided-at={}",
            digest(disposition.rationale_digest),
            disposition.evidence_digests.len(),
            disposition
                .human_gate_response
                .map_or_else(|| "none".into(), |id| identity(id.0)),
            disposition.decided_at,
        ));
    }
    values
}

fn review_phase(value: d::ReviewPhaseV1) -> &'static str {
    match value {
        d::ReviewPhaseV1::FirstPassComplete => "first_pass_complete",
        d::ReviewPhaseV1::Challenged => "challenged",
        d::ReviewPhaseV1::Disposed => "disposed",
    }
}

fn independence(value: d::IndependenceAssessmentV1) -> &'static str {
    match value {
        d::IndependenceAssessmentV1::Correlated => "correlated",
        d::IndependenceAssessmentV1::NotEstablished => "not_established",
    }
}

fn correlation_dimension(value: d::CorrelationDimensionV1) -> &'static str {
    match value {
        d::CorrelationDimensionV1::ExecutionPrincipal => "execution_principal",
        d::CorrelationDimensionV1::ExecutionProfile => "execution_profile",
        d::CorrelationDimensionV1::ContextManifest => "context_manifest",
        d::CorrelationDimensionV1::Model => "model",
        d::CorrelationDimensionV1::Provider => "provider",
        d::CorrelationDimensionV1::Prompt => "prompt",
        d::CorrelationDimensionV1::Grounding => "grounding",
        d::CorrelationDimensionV1::EvidenceScope => "evidence_scope",
        d::CorrelationDimensionV1::Organization => "organization",
    }
}

fn finding_disposition(value: d::FindingDispositionKindV1) -> &'static str {
    match value {
        d::FindingDispositionKindV1::Accepted => "accepted",
        d::FindingDispositionKindV1::Mitigated => "mitigated",
        d::FindingDispositionKindV1::Tested => "tested",
        d::FindingDispositionKindV1::Deferred => "deferred",
        d::FindingDispositionKindV1::RiskAccepted => "risk_accepted",
        d::FindingDispositionKindV1::Disputed => "disputed",
        d::FindingDispositionKindV1::OutOfScope => "out_of_scope",
    }
}

fn human_gate_summary(
    request: Option<&d::HumanGateRequestV1>,
    response: Option<&d::HumanGateResponseV1>,
) -> Vec<String> {
    let Some(request) = request else {
        return vec!["human gate: not requested".into()];
    };
    let mut values = vec![format!(
        "human gate: {} request={} reviewer={} requested-at={}",
        response.map_or("pending", |value| human_gate_disposition(value.disposition)),
        identity(request.id.0),
        identity(request.reviewer),
        request.requested_at,
    )];
    values.push(format!(
        "human gate material: finding={} decision={} revision={} policy={}",
        digest(request.finding_digest),
        digest(request.decision_digest),
        digest(request.material_revision_digest),
        digest(request.policy_digest),
    ));
    values.push(format!(
        "human gate packet: owner={} artifact={} revision={} sha256={}",
        identity(request.decision_packet.owner_route),
        identity(request.decision_packet.artifact_route),
        request.decision_packet.expected_revision,
        digest(request.decision_packet.expected_sha256),
    ));
    if let Some(response) = response {
        values.push(format!(
            "human gate response: id={} request={} rationale={} responded-at={}",
            identity(response.id.0),
            identity(response.request.0),
            digest(response.rationale_digest),
            response.responded_at,
        ));
        values.push(format!(
            "human gate rationale artifact: owner={} artifact={} revision={} sha256={}",
            identity(response.rationale.owner_route),
            identity(response.rationale.artifact_route),
            response.rationale.expected_revision,
            digest(response.rationale.expected_sha256),
        ));
        values.push(format!(
            "human gate authority: decision={} subject={} epoch={}",
            identity(response.authority.decision),
            identity(response.authority.subject),
            response.authority.epoch,
        ));
    }
    values
}

fn optional_identity(value: Option<[u8; 16]>) -> String {
    value.map_or_else(|| "none".into(), identity)
}

fn optional_digest(value: Option<[u8; 32]>) -> String {
    value.map_or_else(|| "none".into(), digest)
}

fn resolution_summary(value: Option<&d::ResolutionV1>) -> String {
    match value {
        None => "resolution: unresolved".into(),
        Some(d::ResolutionV1::ProvenBool { value, .. }) => {
            format!("resolution: proven bool {value}")
        }
        Some(d::ResolutionV1::AuthorizedChoice {
            frontier, selected, ..
        }) => format!(
            "resolution: authorized choice selected={} retained-frontier={:?}",
            identity(selected.0),
            frontier
                .iter()
                .map(|value| identity(value.0))
                .collect::<Vec<_>>()
        ),
        Some(d::ResolutionV1::Maybe(value)) => format!("resolution: maybe {value:?}"),
        Some(d::ResolutionV1::Mu(value)) => format!("resolution: mu {value:?}"),
    }
}

fn identity(value: [u8; 16]) -> String {
    hex(&value)
}

fn digest(value: [u8; 32]) -> String {
    hex(&value)
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
