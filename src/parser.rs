#[derive(Debug, PartialEq)]
pub enum Reading {
    /// 1-0:0.0.0*255 — meter identification string
    MeterId(String),
    /// 1-0:96.1.0*255 — serial number
    SerialNumber(String),
    /// 1-0:1.8.0*255 — cumulative active energy import (kWh)
    EnergyImport(f64),
    /// 1-0:2.8.0*255 — cumulative active energy export (kWh)
    EnergyExport(f64),
    /// 1-0:16.7.0*255 — instantaneous total active power (W)
    PowerTotal(f64),
    /// 1-0:36.7.0*255 — instantaneous L1 active power (W)
    PowerL1(f64),
    /// 1-0:56.7.0*255 — instantaneous L2 active power (W)
    PowerL2(f64),
    /// 1-0:76.7.0*255 — instantaneous L3 active power (W)
    PowerL3(f64),
    /// 1-0:96.5.0*255 — meter status flags (hex-decoded)
    StatusFlags(u32),
    /// 0-0:96.8.0*255 — operating time in seconds (hex-decoded)
    OperatingTime(u32),
    /// Unrecognised OBIS code
    Unknown { code: String, value: String, unit: Option<String> },
}

impl std::fmt::Display for Reading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reading::MeterId(v)       => write!(f, "Meter ID (0.0.0) = {v}"),
            Reading::SerialNumber(v)  => write!(f, "Serial Number (96.1.0) = {v}"),
            Reading::EnergyImport(v)  => write!(f, "Energy Import (1.8.0) = {v} kWh"),
            Reading::EnergyExport(v)  => write!(f, "Energy Export (2.8.0) = {v} kWh"),
            Reading::PowerTotal(v)    => write!(f, "Power Total (16.7.0) = {v} W"),
            Reading::PowerL1(v)       => write!(f, "Power L1 (36.7.0) = {v} W"),
            Reading::PowerL2(v)       => write!(f, "Power L2 (56.7.0) = {v} W"),
            Reading::PowerL3(v)       => write!(f, "Power L3 (76.7.0) = {v} W"),
            Reading::StatusFlags(v)   => write!(f, "Status Flags (96.5.0) = {v:#010X}"),
            Reading::OperatingTime(v) => write!(f, "Operating Time (96.8.0) = {v} s"),
            Reading::Unknown { code, value, unit: Some(u) } => write!(f, "{code} = {value} {u}"),
            Reading::Unknown { code, value, unit: None }    => write!(f, "{code} = {value}"),
        }
    }
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
    let (code, rest) = line.split_once('(').ok_or_else(err)?;
    let value_part = rest.strip_suffix(')').ok_or_else(err)?;
    let (value_str, unit) = match value_part.split_once('*') {
        Some((v, u)) => (v, Some(u.to_string())),
        None => (value_part, None),
    };
    let reading = match code {
        "1-0:0.0.0*255"  => Reading::MeterId(value_str.to_string()),
        "1-0:96.1.0*255" => Reading::SerialNumber(value_str.to_string()),
        "1-0:1.8.0*255"  => Reading::EnergyImport(value_str.parse().map_err(|_| err())?),
        "1-0:2.8.0*255"  => Reading::EnergyExport(value_str.parse().map_err(|_| err())?),
        "1-0:16.7.0*255" => Reading::PowerTotal(value_str.parse().map_err(|_| err())?),
        "1-0:36.7.0*255" => Reading::PowerL1(value_str.parse().map_err(|_| err())?),
        "1-0:56.7.0*255" => Reading::PowerL2(value_str.parse().map_err(|_| err())?),
        "1-0:76.7.0*255" => Reading::PowerL3(value_str.parse().map_err(|_| err())?),
        "1-0:96.5.0*255" => Reading::StatusFlags(u32::from_str_radix(value_str, 16).map_err(|_| err())?),
        "0-0:96.8.0*255" => Reading::OperatingTime(u32::from_str_radix(value_str, 16).map_err(|_| err())?),
        other => Reading::Unknown { code: other.to_string(), value: value_str.to_string(), unit },
    };
    Ok(reading)
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
    fn parse_energy_import() {
        let t = parse_telegram(SAMPLE).unwrap();
        assert!(t.readings.contains(&Reading::EnergyImport(2714.12830185)));
    }

    #[test]
    fn parse_meter_id() {
        let t = parse_telegram(SAMPLE).unwrap();
        assert!(t.readings.contains(&Reading::MeterId("1EBZ0102861889".to_string())));
    }

    #[test]
    fn parse_status_flags() {
        let t = parse_telegram(SAMPLE).unwrap();
        assert!(t.readings.contains(&Reading::StatusFlags(0x001C0104)));
    }

    #[test]
    fn parse_operating_time() {
        let t = parse_telegram(SAMPLE).unwrap();
        assert!(t.readings.contains(&Reading::OperatingTime(0x02FAB8BF)));
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
