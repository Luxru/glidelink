//! Bounded, nonblocking TCP framing. A partial frame is never discarded.
use super::protocol::HEADER_LEN;
use crate::status::Status;
use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub const MAX_FRAME: usize = 1024;
const MAX_QUEUED_FRAMES: usize = 128;
const MAX_QUEUE_AGE: Duration = Duration::from_secs(1);

struct PendingWrite {
    bytes: Vec<u8>,
    offset: usize,
    queued: Instant,
}

pub struct FramedStream {
    socket: TcpStream,
    header: [u8; 2],
    header_read: usize,
    body: Vec<u8>,
    body_read: usize,
    writes: VecDeque<PendingWrite>,
    status: Status,
}

impl FramedStream {
    pub fn new(socket: TcpStream, status: Status) -> io::Result<Self> {
        socket.set_nodelay(true)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            header: [0; 2],
            header_read: 0,
            body: Vec::new(),
            body_read: 0,
            writes: VecDeque::new(),
            status,
        })
    }

    pub fn queue(&mut self, packet: Vec<u8>) -> io::Result<()> {
        if !(HEADER_LEN..=MAX_FRAME).contains(&packet.len()) {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "invalid frame length",
            ));
        }
        if self.writes.len() >= MAX_QUEUED_FRAMES {
            return Err(io::Error::new(
                ErrorKind::OutOfMemory,
                "input send queue full",
            ));
        }
        let mut bytes = Vec::with_capacity(packet.len() + 2);
        bytes.extend_from_slice(&(packet.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&packet);
        self.writes.push_back(PendingWrite {
            bytes,
            offset: 0,
            queued: Instant::now(),
        });
        Ok(())
    }

    pub fn queue_depth(&self) -> usize {
        self.writes.len()
    }

    pub fn queue_age(&self) -> Duration {
        self.writes
            .front()
            .map(|f| f.queued.elapsed())
            .unwrap_or_default()
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if self.queue_age() >= MAX_QUEUE_AGE {
            return Err(io::Error::new(
                ErrorKind::TimedOut,
                "input send queue stalled",
            ));
        }
        while let Some(frame) = self.writes.front_mut() {
            match self.socket.write(&frame.bytes[frame.offset..]) {
                Ok(0) => return Err(io::Error::from(ErrorKind::WriteZero)),
                Ok(n) => {
                    frame.offset += n;
                    if frame.offset == frame.bytes.len() {
                        self.status.add_sent(frame.bytes.len());
                        self.writes.pop_front();
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Returns one complete packet, retaining incomplete headers/bodies until
    /// the next poll. Read at most the current frame, so memory stays bounded.
    pub fn read_packet(&mut self) -> io::Result<Option<Vec<u8>>> {
        loop {
            let target = if self.header_read < 2 {
                &mut self.header[self.header_read..]
            } else {
                &mut self.body[self.body_read..]
            };
            match self.socket.read(target) {
                Ok(0) => return Err(io::Error::from(ErrorKind::UnexpectedEof)),
                Ok(n) if self.header_read < 2 => {
                    self.header_read += n;
                    if self.header_read == 2 {
                        let len = u16::from_be_bytes(self.header) as usize;
                        if !(HEADER_LEN..=MAX_FRAME).contains(&len) {
                            return Err(io::Error::new(
                                ErrorKind::InvalidData,
                                "invalid frame length",
                            ));
                        }
                        self.body.resize(len, 0);
                    }
                }
                Ok(n) => {
                    self.body_read += n;
                    if self.body_read == self.body.len() {
                        self.status.add_received(self.body.len() + 2);
                        self.header_read = 0;
                        self.body_read = 0;
                        return Ok(Some(std::mem::take(&mut self.body)));
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(None),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn pair() -> (TcpStream, FramedStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (
            client,
            FramedStream::new(server, Status::default()).unwrap(),
        )
    }

    fn read(stream: &mut FramedStream) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(packet) = stream.read_packet().unwrap() {
                return packet;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    fn read_error(stream: &mut FramedStream) -> ErrorKind {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match stream.read_packet() {
                Err(error) => return error.kind(),
                Ok(None) => {}
                Ok(Some(_)) => panic!("invalid frame was accepted"),
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn split_header_body_and_joined_frames() {
        let (mut sender, mut receiver) = pair();
        let packet = vec![42; HEADER_LEN];
        sender.write_all(&[0]).unwrap();
        assert!(receiver.read_packet().unwrap().is_none());
        sender.write_all(&[HEADER_LEN as u8]).unwrap();
        sender.write_all(&packet[..3]).unwrap();
        assert!(receiver.read_packet().unwrap().is_none());
        let mut rest = packet[3..].to_vec();
        rest.extend_from_slice(&(HEADER_LEN as u16).to_be_bytes());
        rest.extend_from_slice(&packet);
        sender.write_all(&rest).unwrap();
        assert_eq!(read(&mut receiver), packet);
        assert_eq!(read(&mut receiver), packet);
    }

    #[test]
    fn oversized_frame_and_mid_frame_disconnect_fail() {
        let (mut sender, mut receiver) = pair();
        sender
            .write_all(&((MAX_FRAME + 1) as u16).to_be_bytes())
            .unwrap();
        assert_eq!(read_error(&mut receiver), ErrorKind::InvalidData);

        let (mut sender, mut receiver) = pair();
        sender
            .write_all(&(HEADER_LEN as u16).to_be_bytes())
            .unwrap();
        sender.write_all(&[1, 2]).unwrap();
        drop(sender);
        assert_eq!(read_error(&mut receiver), ErrorKind::UnexpectedEof);
    }

    #[test]
    fn backpressure_is_bounded_and_stalls_fail_closed() {
        let (_sender, mut stream) = pair();
        for _ in 0..MAX_QUEUED_FRAMES {
            stream.queue(vec![0; HEADER_LEN]).unwrap();
        }
        assert!(stream.queue(vec![0; HEADER_LEN]).is_err());
        stream.writes.front_mut().unwrap().queued = Instant::now() - MAX_QUEUE_AGE;
        assert_eq!(stream.flush().unwrap_err().kind(), ErrorKind::TimedOut);
    }
}
