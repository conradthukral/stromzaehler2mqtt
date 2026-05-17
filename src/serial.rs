use std::io::{self, BufRead};
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
    let mut reader = io::BufReader::new(&mut port.file);
    let mut buf = Vec::new();
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        if let Some(i) = line.find('/') {
            buf.extend_from_slice(line[i..].as_bytes());
            break;
        }
    }

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buf.extend_from_slice(line.as_bytes());
        if line.starts_with('!') {
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
        // Re-enable canonical mode so the kernel buffers by \n, letting us
        // read one line at a time without manual chunk scanning. ICRNL stays
        // off (cleared by cfmakeraw) so \r\n arrives as-is; parser trims lines.
        // VEOL='!' alone can't serve as the sole terminator because \n is an
        // unconditional boundary — we detect '!' at the application level instead.
        tios.c_lflag |= libc::ICANON;
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
