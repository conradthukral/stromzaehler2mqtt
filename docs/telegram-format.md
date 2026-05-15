# Telegram format — eBZ DD3 / EN62056-21 mode D

## Serial port settings

| Parameter  | Value |
|------------|-------|
| Baud rate  | 9600  |
| Data bits  | 7     |
| Parity     | Even  |
| Stop bits  | 1     |

The optical read/write head (IEC 62056-21) is typically exposed as `/dev/ttyUSB0` on Linux.

## Telegram structure

The meter emits one telegram roughly every second. Each telegram is ASCII text:

```
/EBZ5DD32R06ETA_107\r\n
\r\n
1-0:0.0.0*255(1EBZ0102861889)\r\n
1-0:1.8.0*255(002714.12830185*kWh)\r\n
...
!\r\n
```

| Section       | Description                                                   |
|---------------|---------------------------------------------------------------|
| `/XXXXXXX`    | Identification line — slash followed by manufacturer + model  |
| `\r\n`        | Blank line separating header from data                        |
| Data lines    | One OBIS reading per line (see below)                         |
| `!`           | End marker. This meter emits no CRC after `!`.                |

The eBZ DD3 does **not** append a CRC after `!`. Some other EN62056-21 meters do; the parser ignores any bytes between `!` and the next `\r\n`.

## Data line format

```
<obis-code>(<value>)
<obis-code>(<value>*<unit>)
```

Examples:
```
1-0:1.8.0*255(002714.12830185*kWh)
1-0:96.5.0*255(001C0104)
```

The OBIS code follows the structure `A-B:C.D.E*F`:

| Field | Meaning                              |
|-------|--------------------------------------|
| A     | Medium (1 = electricity, 0 = abstract)|
| B     | Channel (0 = no channel)             |
| C     | Physical quantity (see table below)  |
| D     | Measurement type                     |
| E     | Tariff / phase                       |
| F     | Storage number (255 = not used)      |

## OBIS codes emitted by the eBZ DD3

| OBIS code      | Description                        | Unit    |
|----------------|------------------------------------|---------|
| `1-0:0.0.0*255`  | Device address                   | —       |
| `1-0:96.1.0*255` | Meter ID                         | —       |
| `1-0:1.8.0*255`  | Active energy import (+A), total | kWh     |
| `1-0:2.8.0*255`  | Active energy export (−A), total | kWh     |
| `1-0:16.7.0*255` | Active power, total (sum L1+L2+L3)| W      |
| `1-0:36.7.0*255` | Active power L1                  | W       |
| `1-0:56.7.0*255` | Active power L2                  | W       |
| `1-0:76.7.0*255` | Active power L3                  | W       |
| `1-0:96.5.0*255` | Meter status flags               | hex     |
| `0-0:96.8.0*255` | Operating time counter           | hex     |

## Notes

- `1-0:96.5.0*255` status field is a hex-encoded bitmask; `001C0104` is the observed idle value.
- `0-0:96.8.0*255` increments with each telegram and appears to be an uptime counter in seconds (hex).
- The meter ID appears in both `0.0.0` and `96.1.0`; they have always been identical in practice.
- Values are zero-padded strings, not floats — parse with a decimal library if exact arithmetic matters.
