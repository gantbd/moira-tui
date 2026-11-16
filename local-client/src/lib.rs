//! Bounded reconnectable private Unix clients for Moira's terminal profiles.
#![forbid(unsafe_code)]

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Duration,
};

use moira_client::{
    AgentParticipationServicePort, AgentServicePort, DeliberationServicePort, SagaServicePort,
};
use moira_protocol::{SagaCommandV1, SagaReplyV1, agent, agent_participation, deliberation};
use moira_wire::{
    LimitsV1, decode_agent_participation_reply, decode_agent_reply, decode_deliberation_reply,
    decode_ready, decode_saga_reply, encode_agent_participation_request, encode_agent_request,
    encode_deliberation_request, encode_saga_request,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientLimits {
    pub max_frame_bytes: usize,
    pub max_requests: usize,
    pub io_timeout: Duration,
}

impl Default for ClientLimits {
    fn default() -> Self {
        Self {
            max_frame_bytes: 64 * 1024,
            max_requests: 1_024,
            io_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientError {
    InvalidLimits,
    NotSent(io::ErrorKind),
    OutcomeUnknown(io::ErrorKind),
    InvalidReady,
    InvalidReply,
    RequestLimit,
}

struct LocalConnection {
    stream: Option<UnixStream>,
    endpoint: PathBuf,
    client_limits: ClientLimits,
    wire_limits: LimitsV1,
    requests_left: usize,
    connection_requests_left: usize,
}

pub struct LocalSagaPort {
    connection: LocalConnection,
}

pub struct LocalAgentPort {
    connection: LocalConnection,
}

pub struct LocalAgentParticipationPort {
    connection: LocalConnection,
}

pub struct LocalDeliberationPort {
    connection: LocalConnection,
}

impl LocalDeliberationPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        Ok(Self {
            connection: LocalConnection::connect(path, limits)?,
        })
    }
}

impl LocalAgentParticipationPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        Ok(Self {
            connection: LocalConnection::connect(path, limits)?,
        })
    }
}

impl LocalAgentPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        Ok(Self {
            connection: LocalConnection::connect(path, limits)?,
        })
    }
}

impl LocalSagaPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        Ok(Self {
            connection: LocalConnection::connect(path, limits)?,
        })
    }
}

impl LocalConnection {
    fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        let endpoint = path.as_ref().to_path_buf();
        let (stream, wire_limits, connection_requests_left) = open_connection(&endpoint, limits)?;
        Ok(Self {
            stream: Some(stream),
            endpoint,
            client_limits: limits,
            wire_limits,
            requests_left: limits.max_requests,
            connection_requests_left,
        })
    }

    fn ensure_connected(&mut self) -> Result<(), ClientError> {
        if self.stream.is_some() && self.connection_requests_left > 0 {
            return Ok(());
        }
        let (stream, wire_limits, requests_left) =
            open_connection(&self.endpoint, self.client_limits)?;
        self.stream = Some(stream);
        self.wire_limits = wire_limits;
        self.connection_requests_left = requests_left;
        Ok(())
    }

    fn prepare(&mut self) -> Result<LimitsV1, ClientError> {
        if self.requests_left == 0 {
            return Err(ClientError::RequestLimit);
        }
        self.ensure_connected()?;
        Ok(self.wire_limits)
    }

    fn exchange(&mut self, bytes: &[u8]) -> Result<Vec<u8>, ClientError> {
        // Once transmission begins the peer may have received the command even
        // if the local write reports an error. Consume both budgets before the
        // write so uncertainty cannot create request capacity on reconnect.
        self.requests_left -= 1;
        self.connection_requests_left -= 1;
        let stream = self.stream.as_mut().expect("prepared connection");
        if let Err(error) = write_frame(stream, bytes) {
            self.invalidate();
            return Err(ClientError::OutcomeUnknown(error.kind()));
        }
        let stream = self.stream.as_mut().expect("prepared connection");
        match read_frame(stream, self.wire_limits.max_frame_bytes) {
            Ok(value) => Ok(value),
            Err(error) => {
                self.invalidate();
                Err(ClientError::OutcomeUnknown(error.kind()))
            }
        }
    }

    fn invalidate(&mut self) {
        self.stream = None;
        self.connection_requests_left = 0;
    }
}

fn open_connection(
    path: impl AsRef<Path>,
    limits: ClientLimits,
) -> Result<(UnixStream, LimitsV1, usize), ClientError> {
    if limits.max_frame_bytes == 0 || limits.max_requests == 0 || limits.io_timeout.is_zero() {
        return Err(ClientError::InvalidLimits);
    }
    let mut stream = UnixStream::connect(path).map_err(|e| ClientError::NotSent(e.kind()))?;
    stream
        .set_read_timeout(Some(limits.io_timeout))
        .and_then(|()| stream.set_write_timeout(Some(limits.io_timeout)))
        .map_err(|e| ClientError::NotSent(e.kind()))?;
    let local = LimitsV1 {
        max_frame_bytes: limits.max_frame_bytes,
    };
    let ready_bytes = read_frame(&mut stream, limits.max_frame_bytes)
        .map_err(|e| ClientError::NotSent(e.kind()))?;
    let ready = decode_ready(&ready_bytes, local).map_err(|_| ClientError::InvalidReady)?;
    let frame = limits.max_frame_bytes.min(ready.max_frame_bytes as usize);
    let requests = limits.max_requests.min(ready.max_requests as usize);
    Ok((
        stream,
        LimitsV1 {
            max_frame_bytes: frame,
        },
        requests,
    ))
}

impl SagaServicePort for LocalSagaPort {
    type Error = ClientError;

    fn execute(&mut self, command: SagaCommandV1) -> Result<SagaReplyV1, Self::Error> {
        let limits = self.connection.prepare()?;
        let bytes = encode_saga_request(&command, limits)
            .map_err(|_| ClientError::NotSent(io::ErrorKind::InvalidData))?;
        let reply = self.connection.exchange(&bytes)?;
        match decode_saga_reply(&reply, limits) {
            Ok(value) => Ok(value),
            Err(_) => {
                self.connection.invalidate();
                Err(ClientError::InvalidReply)
            }
        }
    }
}

impl AgentServicePort for LocalAgentPort {
    type Error = ClientError;

    fn execute(&mut self, command: agent::CommandV1) -> Result<agent::ReplyV1, Self::Error> {
        let limits = self.connection.prepare()?;
        let bytes = encode_agent_request(&command, limits)
            .map_err(|_| ClientError::NotSent(io::ErrorKind::InvalidData))?;
        let reply = self.connection.exchange(&bytes)?;
        decode_agent_reply(&reply, limits).map_err(|_| {
            self.connection.invalidate();
            ClientError::InvalidReply
        })
    }
}

impl AgentParticipationServicePort for LocalAgentParticipationPort {
    type Error = ClientError;

    fn execute_participation(
        &mut self,
        command: agent_participation::CommandV1,
    ) -> Result<agent_participation::ReplyV1, Self::Error> {
        let limits = self.connection.prepare()?;
        let bytes = encode_agent_participation_request(&command, limits)
            .map_err(|_| ClientError::NotSent(io::ErrorKind::InvalidData))?;
        let reply = self.connection.exchange(&bytes)?;
        decode_agent_participation_reply(&reply, limits).map_err(|_| {
            self.connection.invalidate();
            ClientError::InvalidReply
        })
    }
}

impl DeliberationServicePort for LocalDeliberationPort {
    type Error = ClientError;

    fn execute(
        &mut self,
        command: deliberation::CommandV1,
    ) -> Result<deliberation::ReplyV1, Self::Error> {
        let limits = self.connection.prepare()?;
        let bytes = encode_deliberation_request(&command, limits)
            .map_err(|_| ClientError::NotSent(io::ErrorKind::InvalidData))?;
        let reply = self.connection.exchange(&bytes)?;
        decode_deliberation_reply(&reply, limits).map_err(|_| {
            self.connection.invalidate();
            ClientError::InvalidReply
        })
    }
}

fn write_frame(stream: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame length"))?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(bytes)?;
    stream.flush()
}

fn read_frame(stream: &mut UnixStream, maximum: usize) -> io::Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header)?;
    let length = usize::try_from(u32::from_be_bytes(header))
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame length"))?;
    if length == 0 || length > maximum {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame limit"));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| io::Error::other("frame capacity"))?;
    bytes.resize(length, 0);
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moira_client::{
        AgentParticipationCancellationClient, AgentParticipationPermissionClient,
        AgentParticipationPromptClient, AgentParticipationReadOnlyClient, AgentReadOnlyClient,
        DeliberationReadOnlyClient,
    };
    use moira_protocol::agent::{
        CapabilitiesV1, CommandV1, DigestV1, OutcomeV1, PhaseV1, ReplyV1, SessionIdV1,
        SessionViewV1,
    };
    use std::{
        fs,
        os::unix::net::UnixListener,
        sync::atomic::{AtomicU64, Ordering},
    };

    fn test_connection(stream: UnixStream) -> LocalConnection {
        LocalConnection {
            stream: Some(stream),
            endpoint: PathBuf::new(),
            client_limits: ClientLimits::default(),
            wire_limits: LimitsV1::default(),
            requests_left: 1,
            connection_requests_left: 1,
        }
    }

    fn socket_path(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "moira-tui-{label}-{}-{}.sock",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn send_ready(stream: &mut UnixStream, requests: u32) {
        let bytes = moira_wire::encode_ready(
            moira_wire::ReadyV1 {
                protocol_version: 1,
                max_frame_bytes: 64 * 1024,
                max_requests: requests,
            },
            LimitsV1::default(),
        )
        .unwrap();
        write_frame(stream, &bytes).unwrap();
    }

    #[test]
    fn local_agent_port_uses_only_the_public_bounded_wire() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let bytes = read_frame(&mut server, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_agent_request(&bytes, LimitsV1::default()),
                Ok(CommandV1::Inspect {
                    session: SessionIdV1([1; 16])
                })
            );
            let reply = ReplyV1::Accepted(OutcomeV1::Session(SessionViewV1 {
                session: SessionIdV1([1; 16]),
                revision: 2,
                phase: PhaseV1::Ready,
                profile_document_digest: DigestV1([3; 32]),
                active_turn: None,
                last_event_cursor: None,
                capabilities: CapabilitiesV1::default(),
            }));
            let bytes = moira_wire::encode_agent_reply(&reply, LimitsV1::default()).unwrap();
            write_frame(&mut server, &bytes).unwrap();
        });
        let port = LocalAgentPort {
            connection: test_connection(client),
        };
        let mut client = AgentReadOnlyClient::new(port);
        assert!(matches!(
            client.inspect(SessionIdV1([1; 16])),
            Ok(ReplyV1::Accepted(OutcomeV1::Session(_)))
        ));
        worker.join().unwrap();
    }

    #[test]
    fn participation_port_uses_its_distinct_public_bounded_wire() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let bytes = read_frame(&mut server, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_agent_participation_request(&bytes, LimitsV1::default()),
                Ok(agent_participation::CommandV1::Participation {
                    participation: agent_participation::ParticipationIdV1([9; 16]),
                })
            );
            let reply = agent_participation::ReplyV1::Accepted(Box::new(
                agent_participation::OutcomeV1::Participation(None),
            ));
            let bytes =
                moira_wire::encode_agent_participation_reply(&reply, LimitsV1::default()).unwrap();
            write_frame(&mut server, &bytes).unwrap();
        });
        let port = LocalAgentParticipationPort {
            connection: test_connection(client),
        };
        let mut client = AgentParticipationReadOnlyClient::new(port);
        assert_eq!(
            client.participation(agent_participation::ParticipationIdV1([9; 16])),
            Ok(agent_participation::ReplyV1::Accepted(Box::new(
                agent_participation::OutcomeV1::Participation(None)
            )))
        );
        worker.join().unwrap();
    }

    #[test]
    fn deliberation_port_uses_its_distinct_public_bounded_wire() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let worker = std::thread::spawn(move || {
            let bytes = read_frame(&mut server, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_deliberation_request(&bytes, LimitsV1::default()),
                Ok(deliberation::CommandV1::Deliberation {
                    deliberation: deliberation::DeliberationIdV1([14; 16]),
                })
            );
            let reply =
                deliberation::ReplyV1::Accepted(deliberation::OutcomeV1::Deliberation(None));
            let bytes = moira_wire::encode_deliberation_reply(&reply, LimitsV1::default()).unwrap();
            write_frame(&mut server, &bytes).unwrap();
        });
        let port = LocalDeliberationPort {
            connection: test_connection(client),
        };
        let mut client = DeliberationReadOnlyClient::new(port);
        assert_eq!(
            client.deliberation(deliberation::DeliberationIdV1([14; 16])),
            Ok(deliberation::ReplyV1::Accepted(
                deliberation::OutcomeV1::Deliberation(None)
            ))
        );
        worker.join().unwrap();
    }

    #[test]
    fn prompt_client_sends_only_participation_and_exact_bytes() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let participation = agent_participation::ParticipationIdV1([44; 16]);
        let worker = std::thread::spawn(move || {
            let bytes = read_frame(&mut server, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_agent_participation_request(&bytes, LimitsV1::default()),
                Ok(agent_participation::CommandV1::SubmitPrompt(
                    agent_participation::PromptSubmissionV1 {
                        participation,
                        prompt: b"exact prompt bytes".to_vec(),
                    }
                ))
            );
            let reply = agent_participation::ReplyV1::Refused(
                agent_participation::RefusalV1::InvalidOperation,
            );
            let bytes =
                moira_wire::encode_agent_participation_reply(&reply, LimitsV1::default()).unwrap();
            write_frame(&mut server, &bytes).unwrap();
        });
        let port = LocalAgentParticipationPort {
            connection: test_connection(client),
        };
        let mut client = AgentParticipationPromptClient::new(port);
        assert_eq!(
            client.submit(participation, b"exact prompt bytes".to_vec()),
            Ok(agent_participation::ReplyV1::Refused(
                agent_participation::RefusalV1::InvalidOperation
            ))
        );
        worker.join().unwrap();
    }

    #[test]
    fn confirmed_permission_uses_only_the_bounded_participation_wire() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let selection = agent_participation::ConfirmedPermissionSelectionV1 {
            participation: agent_participation::ParticipationIdV1([9; 16]),
            confirmation_digest: [10; 32],
            option: agent_participation::PermissionOptionIdV1([11; 16]),
        };
        let worker = std::thread::spawn(move || {
            let bytes = read_frame(&mut server, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_agent_participation_request(&bytes, LimitsV1::default()),
                Ok(agent_participation::CommandV1::ConfirmPermission(selection))
            );
            let reply = agent_participation::ReplyV1::Refused(
                agent_participation::RefusalV1::RevisionConflict,
            );
            let bytes =
                moira_wire::encode_agent_participation_reply(&reply, LimitsV1::default()).unwrap();
            write_frame(&mut server, &bytes).unwrap();
        });
        let port = LocalAgentParticipationPort {
            connection: test_connection(client),
        };
        let mut client = AgentParticipationPermissionClient::new(port);
        assert_eq!(
            client.confirm(selection),
            Ok(agent_participation::ReplyV1::Refused(
                agent_participation::RefusalV1::RevisionConflict
            ))
        );
        worker.join().unwrap();
    }

    #[test]
    fn confirmed_cancel_uses_only_the_bounded_participation_wire() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let confirmation = agent_participation::ConfirmedCancelV1 {
            participation: agent_participation::ParticipationIdV1([12; 16]),
            confirmation_digest: [13; 32],
        };
        let worker = std::thread::spawn(move || {
            let bytes = read_frame(&mut server, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_agent_participation_request(&bytes, LimitsV1::default()),
                Ok(agent_participation::CommandV1::ConfirmCancel(confirmation))
            );
            let reply = agent_participation::ReplyV1::Refused(
                agent_participation::RefusalV1::RevisionConflict,
            );
            let bytes =
                moira_wire::encode_agent_participation_reply(&reply, LimitsV1::default()).unwrap();
            write_frame(&mut server, &bytes).unwrap();
        });
        let port = LocalAgentParticipationPort {
            connection: test_connection(client),
        };
        let mut client = AgentParticipationCancellationClient::new(port);
        assert_eq!(
            client.confirm(confirmation),
            Ok(agent_participation::ReplyV1::Refused(
                agent_participation::RefusalV1::RevisionConflict
            ))
        );
        worker.join().unwrap();
    }

    #[test]
    fn participation_reconnects_only_for_the_explicit_read_after_an_unknown_mutation() {
        let path = socket_path("participation-reconnect");
        let listener = UnixListener::bind(&path).unwrap();
        let participation = agent_participation::ParticipationIdV1([71; 16]);
        let worker = std::thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            send_ready(&mut first, 4);
            let bytes = read_frame(&mut first, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_agent_participation_request(&bytes, LimitsV1::default()),
                Ok(agent_participation::CommandV1::SubmitPrompt(
                    agent_participation::PromptSubmissionV1 {
                        participation,
                        prompt: b"commit may have happened".to_vec(),
                    }
                ))
            );
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            send_ready(&mut second, 4);
            let bytes = read_frame(&mut second, 64 * 1024).unwrap();
            assert_eq!(
                moira_wire::decode_agent_participation_request(&bytes, LimitsV1::default()),
                Ok(agent_participation::CommandV1::Participation { participation })
            );
            let reply = agent_participation::ReplyV1::Accepted(Box::new(
                agent_participation::OutcomeV1::Participation(None),
            ));
            let bytes =
                moira_wire::encode_agent_participation_reply(&reply, LimitsV1::default()).unwrap();
            write_frame(&mut second, &bytes).unwrap();
        });

        let port = LocalAgentParticipationPort::connect(
            &path,
            ClientLimits {
                max_requests: 4,
                ..ClientLimits::default()
            },
        )
        .unwrap();
        let mut prompt = AgentParticipationPromptClient::new(port);
        assert!(matches!(
            prompt.submit(participation, b"commit may have happened".to_vec()),
            Err(ClientError::OutcomeUnknown(_))
        ));
        let mut reader = AgentParticipationReadOnlyClient::new(prompt.into_port());
        assert_eq!(
            reader.participation(participation),
            Ok(agent_participation::ReplyV1::Accepted(Box::new(
                agent_participation::OutcomeV1::Participation(None)
            )))
        );

        worker.join().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn agent_reconnects_after_the_server_connection_budget_without_resetting_its_global_bound() {
        let path = socket_path("agent-request-bound");
        let listener = UnixListener::bind(&path).unwrap();
        let session = SessionIdV1([81; 16]);
        let worker = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                send_ready(&mut stream, 1);
                let bytes = read_frame(&mut stream, 64 * 1024).unwrap();
                assert_eq!(
                    moira_wire::decode_agent_request(&bytes, LimitsV1::default()),
                    Ok(CommandV1::Inspect { session })
                );
                let reply = ReplyV1::Accepted(OutcomeV1::Session(SessionViewV1 {
                    session,
                    revision: 2,
                    phase: PhaseV1::Ready,
                    profile_document_digest: DigestV1([82; 32]),
                    active_turn: None,
                    last_event_cursor: None,
                    capabilities: CapabilitiesV1::default(),
                }));
                let bytes = moira_wire::encode_agent_reply(&reply, LimitsV1::default()).unwrap();
                write_frame(&mut stream, &bytes).unwrap();
            }
        });

        let port = LocalAgentPort::connect(
            &path,
            ClientLimits {
                max_requests: 2,
                ..ClientLimits::default()
            },
        )
        .unwrap();
        let mut reader = AgentReadOnlyClient::new(port);
        for _ in 0..2 {
            assert!(matches!(
                reader.inspect(session),
                Ok(ReplyV1::Accepted(OutcomeV1::Session(_)))
            ));
        }
        assert_eq!(reader.inspect(session), Err(ClientError::RequestLimit));

        worker.join().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn uncertain_transmission_consumes_the_global_request_budget() {
        let (client, server) = UnixStream::pair().unwrap();
        drop(server);
        let port = LocalAgentPort {
            connection: test_connection(client),
        };
        let session = SessionIdV1([91; 16]);
        let mut reader = AgentReadOnlyClient::new(port);
        assert!(matches!(
            reader.inspect(session),
            Err(ClientError::OutcomeUnknown(_))
        ));
        assert_eq!(reader.inspect(session), Err(ClientError::RequestLimit));
    }
}
