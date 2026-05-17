use std::io;
use std::io::Read;
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
    let mut buf = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let n = port.file.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        if let Some(i) = chunk[..n].iter().position(|&b| b == b'/') {
            buf.extend_from_slice(&chunk[i..n]);
            break;
        }
    }

    while !buf.contains(&b'!') {
        let n = port.file.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        buf.extend_from_slice(&chunk[..n]);
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
