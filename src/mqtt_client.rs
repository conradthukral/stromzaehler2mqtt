use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

pub struct MqttClient {
    stream: TcpStream,
}

impl MqttClient {
    pub fn connect(host: &str, port: u16, client_id: &str) -> std::io::Result<Self> {
        let stream = TcpStream::connect((host, port))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut client = Self { stream };
        client.send_connect(client_id)?;
        client.recv_connack()?;
        Ok(client)
    }

    fn send_connect(&mut self, client_id: &str) -> std::io::Result<()> {
        let pkt = connect_packet(client_id);
        self.stream.write_all(&pkt)
    }

    fn recv_connack(&mut self) -> std::io::Result<()> {
        let mut buf = [0u8; 4];
        self.stream.read_exact(&mut buf)?;
        parse_connack(buf)
    }

    pub fn publish(&mut self, topic: &str, payload: &[u8], retain: bool) -> std::io::Result<()> {
        let pkt = publish_packet(topic, payload, retain);
        self.stream.write_all(&pkt)
    }
}

fn connect_packet(client_id: &str) -> Vec<u8> {
    let id = client_id.as_bytes();
    // Variable header: 6 (protocol name+len) + 1 (level) + 1 (flags) + 2 (keepalive) = 10
    // Payload: 2 (id length prefix) + id bytes
    let remaining = 10 + 2 + id.len();
    let mut pkt = Vec::with_capacity(2 + remaining);
    pkt.push(0x10); // CONNECT
    encode_remaining_len(&mut pkt, remaining);
    // Protocol name "MQTT", level 4 (3.1.1), flags 0x02 (clean session), keep-alive 0
    pkt.extend_from_slice(&[0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x02, 0x00, 0x00]);
    pkt.push((id.len() >> 8) as u8);
    pkt.push(id.len() as u8);
    pkt.extend_from_slice(id);
    pkt
}

fn parse_connack(buf: [u8; 4]) -> std::io::Result<()> {
    if buf[0] != 0x20 || buf[1] != 0x02 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("expected CONNACK, got {:#04x}", buf[0]),
        ));
    }
    if buf[3] != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            format!("CONNACK return code {}", buf[3]),
        ));
    }
    Ok(())
}

fn publish_packet(topic: &str, payload: &[u8], retain: bool) -> Vec<u8> {
    let topic_bytes = topic.as_bytes();
    let remaining = 2 + topic_bytes.len() + payload.len();
    let first = if retain { 0x31u8 } else { 0x30u8 };
    let mut pkt = Vec::with_capacity(2 + remaining);
    pkt.push(first);
    encode_remaining_len(&mut pkt, remaining);
    pkt.push((topic_bytes.len() >> 8) as u8);
    pkt.push(topic_bytes.len() as u8);
    pkt.extend_from_slice(topic_bytes);
    pkt.extend_from_slice(payload);
    pkt
}

fn encode_remaining_len(buf: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut b = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            b |= 0x80;
        }
        buf.push(b);
        if len == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_remaining_len_single_byte() {
        let mut buf = Vec::new();
        encode_remaining_len(&mut buf, 0);
        assert_eq!(buf, &[0x00]);

        buf.clear();
        encode_remaining_len(&mut buf, 127);
        assert_eq!(buf, &[0x7f]);
    }

    #[test]
    fn encode_remaining_len_two_bytes() {
        let mut buf = Vec::new();
        encode_remaining_len(&mut buf, 128);
        assert_eq!(buf, &[0x80, 0x01]);

        buf.clear();
        encode_remaining_len(&mut buf, 321);
        assert_eq!(buf, &[0xc1, 0x02]);
    }

    #[test]
    fn connect_packet_structure() {
        let pkt = connect_packet("test");

        assert_eq!(pkt[0], 0x10); // CONNECT fixed header
        assert_eq!(&pkt[2..8], &[0x00, 0x04, b'M', b'Q', b'T', b'T']); // protocol name
        assert_eq!(pkt[8], 0x04); // protocol level 3.1.1
        assert_eq!(pkt[9], 0x02); // clean session
        assert_eq!(&pkt[10..12], &[0x00, 0x00]); // keep-alive 0
        assert_eq!(&pkt[12..], &[0x00, 0x04, b't', b'e', b's', b't']);
    }

    #[test]
    fn publish_packet_no_retain() {
        let topic = "foo/bar";
        let payload = b"hello";
        let pkt = publish_packet(topic, payload, false);

        assert_eq!(pkt[0], 0x30); // PUBLISH, QoS 0, no retain
        let topic_len = ((pkt[2] as usize) << 8) | pkt[3] as usize;
        assert_eq!(topic_len, topic.len());
        assert_eq!(&pkt[4..4 + topic_len], topic.as_bytes());
        assert_eq!(&pkt[4 + topic_len..], payload);
    }

    #[test]
    fn publish_packet_retain() {
        assert_eq!(
            publish_packet("a/b", b"1", true),
            &[0x31, 0x06, 0x00, 0x03, b'a', b'/', b'b', b'1']
        );
    }

    #[test]
    fn parse_connack_accepts_success_packet() {
        assert!(parse_connack([0x20, 0x02, 0x00, 0x00]).is_ok());
    }

    #[test]
    fn parse_connack_rejects_invalid_header() {
        let err = parse_connack([0x21, 0x02, 0x00, 0x00]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        let err = parse_connack([0x20, 0x03, 0x00, 0x00]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn parse_connack_rejects_broker_refusal() {
        let err = parse_connack([0x20, 0x02, 0x00, 0x05]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionRefused);
    }
}
