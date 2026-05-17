use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::{AsRawFd, RawFd};

pub struct SerialPort {
    pub file: std::fs::File,
    pub fd: RawFd,
}

pub fn open_serial_port(path: &str, baud_rate: u32) -> io::Result<SerialPort> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY)
        .open(path)?;
    let fd = file.as_raw_fd();
    configure_tty(fd, baud_rate)?;
    Ok(SerialPort { file, fd })
}

pub fn read_telegram(port: &mut SerialPort) -> io::Result<Vec<u8>> {
    unsafe { libc::tcflush(port.fd, libc::TCIFLUSH) };
    read_framed_telegram(&mut port.file)
}

fn read_framed_telegram<R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        if reader.read(&mut byte)? == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        let b = byte[0];
        if b == b'/' {
            buf.push(b);
            break;
        }
    }

    loop {
        if reader.read(&mut byte)? == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        let b = byte[0];
        if b == b'/' {
            buf.clear();
            buf.push(b);
            continue;
        }
        buf.push(b);
        if b == b'!' {
            break;
        }
    }

    Ok(buf)
}

fn configure_tty(fd: RawFd, baud_rate: u32) -> io::Result<()> {
    let speed = match baud_rate {
        300 => libc::B300,
        600 => libc::B600,
        1200 => libc::B1200,
        2400 => libc::B2400,
        4800 => libc::B4800,
        9600 => libc::B9600,
        19200 => libc::B19200,
        38400 => libc::B38400,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported baud rate: {baud_rate}"),
            ));
        }
    };
    unsafe {
        let mut tios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut tios) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut tios);
        // 7E1: 7 data bits, even parity, 1 stop bit (EN62056-21 mode D)
        tios.c_cflag &= !(libc::CSIZE | libc::PARODD | libc::CSTOPB);
        tios.c_cflag |= libc::CS7 | libc::PARENB | libc::CREAD | libc::CLOCAL;
        libc::cfsetspeed(&mut tios, speed);
        if libc::tcsetattr(fd, libc::TCSANOW, &tios) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framed_read_resyncs_on_slash_inside_partial_line() {
        let mut input = io::Cursor::new(
            b"1-0:36.7.0*255(0001/EBZ5DD32R06ETA_107\r\n\
              1-0:1.8.0*255(000001.00000000*kWh)\r\n\
              !\r\n\
              /EBZ5DD32R06ETA_107\r\n\
              1-0:1.8.0*255(000002.00000000*kWh)\r\n\
              !\r\n",
        );

        let telegram = read_framed_telegram(&mut input).unwrap();

        assert_eq!(
            telegram,
            b"/EBZ5DD32R06ETA_107\r\n\
              1-0:1.8.0*255(000001.00000000*kWh)\r\n\
              !"
        );
    }

    #[test]
    fn framed_read_does_not_consume_next_telegram() {
        let mut input = io::Cursor::new(
            b"/first\r\n\
              1-0:1.8.0*255(000001.00000000*kWh)\r\n\
              !\r\n\
              /second\r\n\
              1-0:1.8.0*255(000002.00000000*kWh)\r\n\
              !\r\n",
        );

        let first = read_framed_telegram(&mut input).unwrap();
        let second = read_framed_telegram(&mut input).unwrap();

        assert_eq!(
            first,
            b"/first\r\n\
              1-0:1.8.0*255(000001.00000000*kWh)\r\n\
              !"
        );
        assert_eq!(
            second,
            b"/second\r\n\
              1-0:1.8.0*255(000002.00000000*kWh)\r\n\
              !"
        );
    }

    #[test]
    fn framed_read_resyncs_on_header_inside_partial_telegram() {
        let mut input = io::Cursor::new(
            b"/stale\r\n\
              1-0:36.7.0*255(00/EBZ5DD32R06ETA_107\r\n\
              1-0:1.8.0*255(000002.00000000*kWh)\r\n\
              !\r\n",
        );

        let telegram = read_framed_telegram(&mut input).unwrap();

        assert_eq!(
            telegram,
            b"/EBZ5DD32R06ETA_107\r\n\
              1-0:1.8.0*255(000002.00000000*kWh)\r\n\
              !"
        );
    }
}
