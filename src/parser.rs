#[derive(Debug, PartialEq)]
pub struct Reading {
    pub obis: String,
    pub value: String,
    pub unit: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct Telegram {
    pub device_id: String,
    pub readings: Vec<Reading>,
}

#[derive(Debug, PartialEq)]
pub enum ParseError {
    InvalidUtf8,
    MissingHeader,
    MalformedLine(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidUtf8 => write!(f, "invalid UTF-8 in telegram"),
            ParseError::MissingHeader => write!(f, "telegram does not start with '/'"),
            ParseError::MalformedLine(l) => write!(f, "malformed data line: {l:?}"),
        }
    }
}

/// Extract complete telegrams from a streaming byte buffer.
///
/// Returns slices of each complete telegram found and the unconsumed tail
/// (an incomplete telegram in progress). Callers should drain the consumed
/// prefix and retain the tail for the next read.
pub fn split_telegrams(buf: &[u8]) -> (Vec<&[u8]>, &[u8]) {
    let mut telegrams = Vec::new();
    let mut pos = 0;

    while pos < buf.len() {
        let start = match buf[pos..].iter().position(|&b| b == b'/') {
            Some(i) => pos + i,
            None => return (telegrams, &buf[pos..]),
        };

        let end_bang = match buf[start..].iter().position(|&b| b == b'!') {
            Some(i) => start + i,
            None => return (telegrams, &buf[start..]),
        };

        // Consume '!' plus any trailing \r\n (and optional CRC before \n)
        let mut end = end_bang + 1;
        while end < buf.len() && (buf[end] == b'\r' || buf[end] == b'\n') {
            end += 1;
        }

        telegrams.push(&buf[start..end]);
        pos = end;
    }

    (telegrams, &buf[pos..])
}

pub fn parse_telegram(data: &[u8]) -> Result<Telegram, ParseError> {
    let text = std::str::from_utf8(data).map_err(|_| ParseError::InvalidUtf8)?;
    let mut lines = text.lines();

    let header = lines.next().ok_or(ParseError::MissingHeader)?;
    if !header.starts_with('/') {
        return Err(ParseError::MissingHeader);
    }
    let device_id = header[1..].trim().to_string();

    let mut readings = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('!') {
            continue;
        }
        readings.push(parse_data_line(line)?);
    }

    Ok(Telegram { device_id, readings })
}

fn parse_data_line(line: &str) -> Result<Reading, ParseError> {
    let err = || ParseError::MalformedLine(line.to_string());
    let (obis, rest) = line.split_once('(').ok_or_else(err)?;
    let value_part = rest.strip_suffix(')').ok_or_else(err)?;
    let (value, unit) = match value_part.split_once('*') {
        Some((v, u)) => (v.to_string(), Some(u.to_string())),
        None => (value_part.to_string(), None),
    };
    Ok(Reading { obis: obis.to_string(), value, unit })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real telegram captured from an eBZ DD3 2R06 meter
    const SAMPLE: &[u8] = b"/EBZ5DD32R06ETA_107\r\n\
        \r\n\
        1-0:0.0.0*255(1EBZ0102861889)\r\n\
        1-0:96.1.0*255(1EBZ0102861889)\r\n\
        1-0:1.8.0*255(002714.12830185*kWh)\r\n\
        1-0:2.8.0*255(000001.20600000*kWh)\r\n\
        1-0:16.7.0*255(000211.26*W)\r\n\
        1-0:36.7.0*255(000157.64*W)\r\n\
        1-0:56.7.0*255(000015.64*W)\r\n\
        1-0:76.7.0*255(000037.98*W)\r\n\
        1-0:96.5.0*255(001C0104)\r\n\
        0-0:96.8.0*255(02FAB8BF)\r\n\
        !\r\n";

    #[test]
    fn parse_device_id() {
        let t = parse_telegram(SAMPLE).unwrap();
        assert_eq!(t.device_id, "EBZ5DD32R06ETA_107");
    }

    #[test]
    fn parse_reading_count() {
        let t = parse_telegram(SAMPLE).unwrap();
        assert_eq!(t.readings.len(), 10);
    }

    #[test]
    fn parse_reading_with_unit() {
        let t = parse_telegram(SAMPLE).unwrap();
        let r = t.readings.iter().find(|r| r.obis == "1-0:1.8.0*255").unwrap();
        assert_eq!(r.value, "002714.12830185");
        assert_eq!(r.unit, Some("kWh".to_string()));
    }

    #[test]
    fn parse_reading_without_unit() {
        let t = parse_telegram(SAMPLE).unwrap();
        let r = t.readings.iter().find(|r| r.obis == "1-0:0.0.0*255").unwrap();
        assert_eq!(r.value, "1EBZ0102861889");
        assert_eq!(r.unit, None);
    }

    #[test]
    fn split_single_telegram() {
        let (telegrams, remaining) = split_telegrams(SAMPLE);
        assert_eq!(telegrams.len(), 1);
        assert!(remaining.is_empty());
    }

    #[test]
    fn split_two_telegrams() {
        let two: Vec<u8> = [SAMPLE, SAMPLE].concat();
        let (telegrams, remaining) = split_telegrams(&two);
        assert_eq!(telegrams.len(), 2);
        assert!(remaining.is_empty());
    }

    #[test]
    fn split_partial_telegram_at_end() {
        // Buffer ends mid-telegram — incomplete part must be returned as remainder
        let partial = b"/EBZ5DD32R06ETA_107\r\n\r\n1-0:1.8.0*255(002714*kWh)\r\n";
        let full: Vec<u8> = [SAMPLE, partial.as_ref()].concat();
        let (telegrams, remaining) = split_telegrams(&full);
        assert_eq!(telegrams.len(), 1);
        assert_eq!(remaining, partial.as_ref());
    }

    #[test]
    fn malformed_line_missing_parens() {
        let bad = b"/EBZ\r\n\r\n1-0:1.8.0*255 no parens here\r\n!\r\n";
        assert!(matches!(
            parse_telegram(bad),
            Err(ParseError::MalformedLine(_))
        ));
    }

    #[test]
    fn missing_header() {
        assert_eq!(parse_telegram(b""), Err(ParseError::MissingHeader));
        assert_eq!(parse_telegram(b"no slash\r\n!\r\n"), Err(ParseError::MissingHeader));
    }
}
