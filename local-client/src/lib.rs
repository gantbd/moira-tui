//! Bounded private Unix client for Moira's saga service profile.
#![forbid(unsafe_code)]

use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

use moira_client::SagaServicePort;
use moira_protocol::{SagaCommandV1, SagaReplyV1};
use moira_wire::{LimitsV1, decode_ready, decode_saga_reply, encode_saga_request};

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

impl LocalSagaPort {
    pub fn connect(path: impl AsRef<Path>, limits: ClientLimits) -> Result<Self, ClientError> {
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
        if ready.protocol_version != 1 || ready.max_frame_bytes == 0 || ready.max_requests == 0 {
            return Err(ClientError::InvalidReady);
        }
        let negotiated_frame = limits.max_frame_bytes.min(ready.max_frame_bytes as usize);
        let negotiated_requests = limits.max_requests.min(ready.max_requests as usize);
        Ok(Self {
            stream,
            wire_limits: LimitsV1 {
                max_frame_bytes: negotiated_frame,
            },
            requests_left: negotiated_requests,
        })
    }
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
