const TELEGRAM_HEADER_PREFIX: char = '/';
const TELEGRAM_TERMINATOR_PREFIX: char = '!';

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
    Unknown {
        code: String,
        value: String,
        unit: Option<String>,
    },
}

impl std::fmt::Display for Reading {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Reading::MeterId(v) => write!(f, "Meter ID (0.0.0) = {v}"),
            Reading::SerialNumber(v) => write!(f, "Serial Number (96.1.0) = {v}"),
            Reading::EnergyImport(v) => write!(f, "Energy Import (1.8.0) = {v} kWh"),
            Reading::EnergyExport(v) => write!(f, "Energy Export (2.8.0) = {v} kWh"),
            Reading::PowerTotal(v) => write!(f, "Power Total (16.7.0) = {v} W"),
            Reading::PowerL1(v) => write!(f, "Power L1 (36.7.0) = {v} W"),
            Reading::PowerL2(v) => write!(f, "Power L2 (56.7.0) = {v} W"),
            Reading::PowerL3(v) => write!(f, "Power L3 (76.7.0) = {v} W"),
            Reading::StatusFlags(v) => write!(f, "Status Flags (96.5.0) = {v:#010X}"),
            Reading::OperatingTime(v) => write!(f, "Operating Time (96.8.0) = {v} s"),
            Reading::Unknown {
                code,
                value,
                unit: Some(u),
            } => write!(f, "{code} = {value} {u}"),
            Reading::Unknown {
                code,
                value,
                unit: None,
            } => write!(f, "{code} = {value}"),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Telegram {
    pub device_id: String,
    pub readings: Vec<Reading>,
}

impl Telegram {
    pub fn meter_id(&self) -> Option<&str> {
        self.readings.iter().find_map(|reading| match reading {
            Reading::MeterId(value) => Some(value.as_str()),
            _ => None,
        })
    }
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

pub fn parse_telegram(data: &[u8]) -> Result<Telegram, ParseError> {
    let text = std::str::from_utf8(data).map_err(|_| ParseError::InvalidUtf8)?;
    let mut lines = text.lines();

    let header = lines.next().ok_or(ParseError::MissingHeader)?;
    let device_id = parse_device_id(header)?;

    let mut readings = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with(TELEGRAM_TERMINATOR_PREFIX) {
            continue;
        }
        readings.push(parse_reading(line)?);
    }

    Ok(Telegram {
        device_id,
        readings,
    })
}

fn parse_device_id(header: &str) -> Result<String, ParseError> {
    header
        .strip_prefix(TELEGRAM_HEADER_PREFIX)
        .map(|device_id| device_id.trim().to_string())
        .ok_or(ParseError::MissingHeader)
}

fn parse_reading(line: &str) -> Result<Reading, ParseError> {
    let (code, value_str, unit) = split_reading_parts(line)?;
    let reading = match code {
        "1-0:0.0.0*255" => Reading::MeterId(value_str.to_string()),
        "1-0:96.1.0*255" => Reading::SerialNumber(value_str.to_string()),
        "1-0:1.8.0*255" => Reading::EnergyImport(parse_decimal(line, value_str)?),
        "1-0:2.8.0*255" => Reading::EnergyExport(parse_decimal(line, value_str)?),
        "1-0:16.7.0*255" => Reading::PowerTotal(parse_decimal(line, value_str)?),
        "1-0:36.7.0*255" => Reading::PowerL1(parse_decimal(line, value_str)?),
        "1-0:56.7.0*255" => Reading::PowerL2(parse_decimal(line, value_str)?),
        "1-0:76.7.0*255" => Reading::PowerL3(parse_decimal(line, value_str)?),
        "1-0:96.5.0*255" => Reading::StatusFlags(parse_hex(line, value_str)?),
        "0-0:96.8.0*255" => Reading::OperatingTime(parse_hex(line, value_str)?),
        other => Reading::Unknown {
            code: other.to_string(),
            value: value_str.to_string(),
            unit,
        },
    };
    Ok(reading)
}

fn split_reading_parts(line: &str) -> Result<(&str, &str, Option<String>), ParseError> {
    let (code, rest) = line.split_once('(').ok_or_else(|| malformed_line(line))?;
    let value_part = rest.strip_suffix(')').ok_or_else(|| malformed_line(line))?;
    let (value, unit) = match value_part.split_once('*') {
        Some((value, unit)) => (value, Some(unit.to_string())),
        None => (value_part, None),
    };
    Ok((code, value, unit))
}

fn parse_decimal(line: &str, value: &str) -> Result<f64, ParseError> {
    value.parse().map_err(|_| malformed_line(line))
}

fn parse_hex(line: &str, value: &str) -> Result<u32, ParseError> {
    u32::from_str_radix(value, 16).map_err(|_| malformed_line(line))
}

fn malformed_line(line: &str) -> ParseError {
    ParseError::MalformedLine(line.to_string())
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

    fn sample_telegram() -> Telegram {
        parse_telegram(SAMPLE).unwrap()
    }

    fn parse_single_line(line: &str) -> Reading {
        let telegram = format!("/EBZ\r\n{line}\r\n!\r\n");
        let parsed = parse_telegram(telegram.as_bytes()).unwrap();
        assert_eq!(parsed.readings.len(), 1);
        parsed.readings.into_iter().next().unwrap()
    }

    fn assert_sample_contains(expected: Reading) {
        let telegram = sample_telegram();
        assert!(telegram.readings.contains(&expected));
    }

    #[test]
    fn parse_device_id() {
        let t = sample_telegram();
        assert_eq!(t.device_id, "EBZ5DD32R06ETA_107");
    }

    #[test]
    fn parse_reading_count() {
        let t = sample_telegram();
        assert_eq!(t.readings.len(), 10);
    }

    #[test]
    fn parse_energy_import() {
        assert_sample_contains(Reading::EnergyImport(2714.12830185));
    }

    #[test]
    fn parse_meter_id() {
        assert_sample_contains(Reading::MeterId("1EBZ0102861889".to_string()));
    }

    #[test]
    fn parse_serial_number() {
        assert_sample_contains(Reading::SerialNumber("1EBZ0102861889".to_string()));
    }

    #[test]
    fn parse_energy_export() {
        assert_sample_contains(Reading::EnergyExport(1.206));
    }

    #[test]
    fn parse_power_total() {
        assert_sample_contains(Reading::PowerTotal(211.26));
    }

    #[test]
    fn parse_power_l1() {
        assert_sample_contains(Reading::PowerL1(157.64));
    }

    #[test]
    fn parse_power_l2() {
        assert_sample_contains(Reading::PowerL2(15.64));
    }

    #[test]
    fn parse_power_l3() {
        assert_sample_contains(Reading::PowerL3(37.98));
    }

    #[test]
    fn parse_status_flags() {
        assert_sample_contains(Reading::StatusFlags(0x001C0104));
    }

    #[test]
    fn parse_operating_time() {
        assert_sample_contains(Reading::OperatingTime(0x02FAB8BF));
    }

    #[test]
    fn parse_unknown_obis_with_unit() {
        assert_eq!(
            parse_single_line("1-0:99.9.9*255(123.45*kvarh)"),
            Reading::Unknown {
                code: "1-0:99.9.9*255".to_string(),
                value: "123.45".to_string(),
                unit: Some("kvarh".to_string()),
            }
        );
    }

    #[test]
    fn parse_unknown_obis_without_unit() {
        assert_eq!(
            parse_single_line("1-0:99.9.9*255(opaque-value)"),
            Reading::Unknown {
                code: "1-0:99.9.9*255".to_string(),
                value: "opaque-value".to_string(),
                unit: None,
            }
        );
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
    fn malformed_line_invalid_float_value() {
        let bad = b"/EBZ\r\n\r\n1-0:1.8.0*255(not-a-float*kWh)\r\n!\r\n";
        assert_eq!(
            parse_telegram(bad),
            Err(ParseError::MalformedLine(
                "1-0:1.8.0*255(not-a-float*kWh)".to_string()
            ))
        );
    }

    #[test]
    fn malformed_line_invalid_hex_value() {
        let bad = b"/EBZ\r\n\r\n1-0:96.5.0*255(nothex)\r\n!\r\n";
        assert_eq!(
            parse_telegram(bad),
            Err(ParseError::MalformedLine(
                "1-0:96.5.0*255(nothex)".to_string()
            ))
        );
    }

    #[test]
    fn invalid_utf8() {
        assert_eq!(
            parse_telegram(b"/EBZ\xff\r\n!\r\n"),
            Err(ParseError::InvalidUtf8)
        );
    }

    #[test]
    fn missing_header() {
        assert_eq!(parse_telegram(b""), Err(ParseError::MissingHeader));
        assert_eq!(
            parse_telegram(b"no slash\r\n!\r\n"),
            Err(ParseError::MissingHeader)
        );
    }

    #[test]
    fn display_reading_variants() {
        assert_eq!(
            Reading::SerialNumber("1EBZ0102861889".to_string()).to_string(),
            "Serial Number (96.1.0) = 1EBZ0102861889"
        );
        assert_eq!(
            Reading::PowerL2(15.64).to_string(),
            "Power L2 (56.7.0) = 15.64 W"
        );
        assert_eq!(
            Reading::Unknown {
                code: "1-0:99.9.9*255".to_string(),
                value: "opaque-value".to_string(),
                unit: Some("kvarh".to_string()),
            }
            .to_string(),
            "1-0:99.9.9*255 = opaque-value kvarh"
        );
        assert_eq!(
            Reading::Unknown {
                code: "1-0:99.9.9*255".to_string(),
                value: "opaque-value".to_string(),
                unit: None,
            }
            .to_string(),
            "1-0:99.9.9*255 = opaque-value"
        );
    }

    #[test]
    fn display_parse_error_variants() {
        assert_eq!(
            ParseError::InvalidUtf8.to_string(),
            "invalid UTF-8 in telegram"
        );
        assert_eq!(
            ParseError::MissingHeader.to_string(),
            "telegram does not start with '/'"
        );
        assert_eq!(
            ParseError::MalformedLine("broken".to_string()).to_string(),
            "malformed data line: \"broken\""
        );
    }
}
