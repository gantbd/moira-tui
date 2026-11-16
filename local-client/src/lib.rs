//! Bounded private Unix client for Moira's saga service profile.
#![forbid(unsafe_code)]

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use moira_client::{AgentParticipationServicePort, AgentServicePort, SagaServicePort};
use moira_protocol::{SagaCommandV1, SagaReplyV1, agent, agent_participation};
use moira_wire::{
    LimitsV1, decode_agent_participation_reply, decode_agent_reply, decode_ready,
    decode_saga_reply, encode_agent_participation_request, encode_agent_request,
    encode_saga_request,
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

pub struct LocalSagaPort {
    stream: UnixStream,
    wire_limits: LimitsV1,
    requests_left: usize,
}

pub struct LocalAgentPort {
    stream: UnixStream,
    wire_limits: LimitsV1,
    requests_left: usize,
}

pub struct LocalAgentParticipationPort {
    stream: UnixStream,
    wire_limits: LimitsV1,
    requests_left: usize,
}

impl LocalAgentParticipationPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        let (stream, wire_limits, requests_left) = connect(path, limits)?;
        Ok(Self {
            stream,
            wire_limits,
            requests_left,
        })
    }
}

impl LocalAgentPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        let (stream, wire_limits, requests_left) = connect(path, limits)?;
        Ok(Self {
            stream,
            wire_limits,
            requests_left,
        })
    }
}

impl LocalSagaPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
        let (stream, wire_limits, requests_left) = connect(path, limits)?;
        Ok(Self {
            stream,
            wire_limits,
            requests_left,
        })
    }
}

fn connect(
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
        if self.requests_left == 0 {
            return Err(ClientError::RequestLimit);
        }
        let bytes = encode_saga_request(&command, self.wire_limits)
            .map_err(|_| ClientError::NotSent(io::ErrorKind::InvalidData))?;
        write_frame(&mut self.stream, &bytes).map_err(|e| ClientError::OutcomeUnknown(e.kind()))?;
        self.requests_left -= 1;
        let reply = read_frame(&mut self.stream, self.wire_limits.max_frame_bytes)
            .map_err(|e| ClientError::OutcomeUnknown(e.kind()))?;
        decode_saga_reply(&reply, self.wire_limits).map_err(|_| ClientError::InvalidReply)
    }
}

impl AgentServicePort for LocalAgentPort {
    type Error = ClientError;

    fn execute(&mut self, command: agent::CommandV1) -> Result<agent::ReplyV1, Self::Error> {
        if self.requests_left == 0 {
            return Err(ClientError::RequestLimit);
        }
        let bytes = encode_agent_request(&command, self.wire_limits)
            .map_err(|_| ClientError::NotSent(io::ErrorKind::InvalidData))?;
        write_frame(&mut self.stream, &bytes).map_err(|e| ClientError::OutcomeUnknown(e.kind()))?;
        self.requests_left -= 1;
        let reply = read_frame(&mut self.stream, self.wire_limits.max_frame_bytes)
            .map_err(|e| ClientError::OutcomeUnknown(e.kind()))?;
        decode_agent_reply(&reply, self.wire_limits).map_err(|_| ClientError::InvalidReply)
    }
}

impl AgentParticipationServicePort for LocalAgentParticipationPort {
    type Error = ClientError;

    fn execute_participation(
        &mut self,
        command: agent_participation::CommandV1,
    ) -> Result<agent_participation::ReplyV1, Self::Error> {
        if self.requests_left == 0 {
            return Err(ClientError::RequestLimit);
        }
        let bytes = encode_agent_participation_request(&command, self.wire_limits)
            .map_err(|_| ClientError::NotSent(io::ErrorKind::InvalidData))?;
        write_frame(&mut self.stream, &bytes).map_err(|e| ClientError::OutcomeUnknown(e.kind()))?;
        self.requests_left -= 1;
        let reply = read_frame(&mut self.stream, self.wire_limits.max_frame_bytes)
            .map_err(|e| ClientError::OutcomeUnknown(e.kind()))?;
        decode_agent_participation_reply(&reply, self.wire_limits)
            .map_err(|_| ClientError::InvalidReply)
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
        AgentParticipationReadOnlyClient, AgentReadOnlyClient,
    };
    use moira_protocol::agent::{
        CapabilitiesV1, CommandV1, DigestV1, OutcomeV1, PhaseV1, ReplyV1, SessionIdV1,
        SessionViewV1,
    };

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
            stream: client,
            wire_limits: LimitsV1::default(),
            requests_left: 1,
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
            stream: client,
            wire_limits: LimitsV1::default(),
            requests_left: 1,
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
            stream: client,
            wire_limits: LimitsV1::default(),
            requests_left: 1,
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
            stream: client,
            wire_limits: LimitsV1::default(),
            requests_left: 1,
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
}
