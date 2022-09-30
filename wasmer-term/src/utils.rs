#![allow(unused_imports)]
use tracing::metadata::LevelFilter;
use tracing::{debug, error, info, instrument, span, trace, warn, Level};
use tracing_subscriber::fmt::SubscriberBuilder;
use tracing_subscriber::EnvFilter;
#[cfg(unix)]
use {
    libc::{
        c_int, tcsetattr, termios, ECHO, ECHONL, ICANON, ICRNL, IEXTEN, ISIG, IXON, OPOST, TCSANOW,
    },
    std::mem,
    std::os::unix::io::AsRawFd,
};

pub fn log_init(verbose: i32, debug: bool) {
    let mut log_level = match verbose {
        0 => None,
        1 => Some(LevelFilter::WARN),
        2 => Some(LevelFilter::INFO),
        3 => Some(LevelFilter::DEBUG),
        4 => Some(LevelFilter::TRACE),
        _ => None,
    };
    if debug {
        log_level = Some(LevelFilter::DEBUG);
    }

    if let Some(log_level) = log_level {
        SubscriberBuilder::default()
            .with_max_level(log_level)
            .init();
    } else {
        SubscriberBuilder::default()
            .with_env_filter(EnvFilter::from_default_env())
            .init();
    }
}

#[cfg(unix)]
pub fn io_result(ret: libc::c_int) -> std::io::Result<()> {
    match ret {
        0 => Ok(()),
        _ => Err(std::io::Error::last_os_error()),
    }
}

#[cfg(unix)]
pub fn set_mode_no_echo() -> std::fs::File {
    let tty = std::fs::File::open("/dev/tty").unwrap();
    let fd = tty.as_raw_fd();

    let mut termios = mem::MaybeUninit::<termios>::uninit();
    io_result(unsafe { ::libc::tcgetattr(fd, termios.as_mut_ptr()) }).unwrap();
    let mut termios = unsafe { termios.assume_init() };

    termios.c_lflag &= !ECHO;
    termios.c_lflag &= !ICANON;
    termios.c_lflag &= !ISIG;
    termios.c_lflag &= !IXON;
    termios.c_lflag &= !IEXTEN;
    termios.c_lflag &= !ICRNL;
    termios.c_lflag &= !OPOST;

    unsafe { tcsetattr(fd, TCSANOW, &termios) };
    tty
}

#[cfg(unix)]
pub fn set_mode_echo() -> std::fs::File {
    let tty = std::fs::File::open("/dev/tty").unwrap();
    let fd = tty.as_raw_fd();

    let mut termios = mem::MaybeUninit::<termios>::uninit();
    io_result(unsafe { ::libc::tcgetattr(fd, termios.as_mut_ptr()) }).unwrap();
    let mut termios = unsafe { termios.assume_init() };

    termios.c_lflag |= ECHO;
    termios.c_lflag |= ICANON;
    termios.c_lflag |= ISIG;
    termios.c_lflag |= IXON;
    termios.c_lflag |= IEXTEN;
    termios.c_lflag |= ICRNL;
    termios.c_lflag |= OPOST;

    unsafe { tcsetattr(fd, TCSANOW, &termios) };
    tty
}
