use super::{Arguments, Tool};
use crate::files::InputFiles;
use libc::SIGPIPE;
use std::borrow::Cow;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU8, Ordering};

pub(super) const IGNORE_SIGPIPE: u8 = 1 << 3;
pub(super) static INVOCATION: AtomicU8 = AtomicU8::new(0);

#[doc(hidden)]
pub extern "C" fn save_invocation() {
    let mut state = 0;
    for fd in 0..=libc::STDERR_FILENO {
        // SAFETY: F_GETFD takes no third argument and does not modify fd.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            state |= 1 << fd;
        }
    }
    let mut action = MaybeUninit::<libc::sigaction>::zeroed();
    // SAFETY: sigaction initializes the output on success. A null input queries the current action.
    if unsafe { libc::sigaction(SIGPIPE, std::ptr::null(), action.as_mut_ptr()) } == 0
        && unsafe { action.assume_init().sa_sigaction } == libc::SIG_IGN
    {
        state |= IGNORE_SIGPIPE;
    }
    INVOCATION.store(state, Ordering::Relaxed);
}

impl Tool {
    fn new(
        executable: &Path,
        name: Option<&'static str>,
        argv0: Option<OsString>,
        launcher_options: Arguments,
        args: env::ArgsOs,
    ) -> io::Result<Self> {
        let name = match name {
            Some(name) => Cow::Borrowed(OsStr::new(name)),
            None => Cow::Owned(
                executable
                    .file_name()
                    .ok_or_else(|| io::Error::other("invalid installed wrapper name"))?
                    .to_owned(),
            ),
        };
        let argv0 = argv0.unwrap_or_else(|| name.to_os_string());
        let mut wrapped_name = OsString::from(".");
        wrapped_name.push(&name);
        wrapped_name.push("-wrapped");
        let vendor = executable.with_file_name(wrapped_name);
        Ok(Self {
            name,
            args: args.map(OsStringExt::into_vec).collect(),
            launcher_options,
            argv0,
            vendor,
            inputs: InputFiles::default(),
        })
    }
}

#[doc(hidden)]
pub enum Invocation {
    Standalone(Option<&'static str>),
    Launcher,
}

#[doc(hidden)]
pub fn wrapper_main(
    invocation: Invocation,
    wrapper: impl FnOnce(Tool) -> io::Result<i32>,
) -> ExitCode {
    let mut args = env::args_os();
    let argv0 = args.next();
    let (launch, name) = match invocation {
        Invocation::Standalone(name) => (
            fs::read_link("/proc/self/exe").map(|path| (path, Vec::new())),
            name,
        ),
        Invocation::Launcher => (read_launcher(&mut args), None),
    };
    let default_label = OsStr::new(name.unwrap_or("tool"));
    let (path, launcher_options) = match launch {
        Ok(launch) => launch,
        Err(error) => return wrapper_exit(Err(error), default_label),
    };
    let label = match invocation {
        Invocation::Launcher => path.file_name().unwrap_or(default_label),
        Invocation::Standalone(_) => default_label,
    };
    wrapper_exit(
        Tool::new(&path, name, argv0, launcher_options, args).and_then(wrapper),
        label,
    )
}

fn read_launcher(args: &mut env::ArgsOs) -> io::Result<(PathBuf, Arguments)> {
    let mut options = Vec::new();
    for arg in args {
        if arg.as_bytes().starts_with(b"--") {
            options.push(arg.into_vec());
        } else {
            let path = PathBuf::from(arg);
            if path.is_absolute() && path.file_name().is_some() {
                return Ok((path, options));
            }
            break;
        }
    }
    Err(io::Error::other("expected an absolute launcher path"))
}

fn wrapper_exit(result: io::Result<i32>, label: &OsStr) -> ExitCode {
    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("{} wrapper: error: {error}", label.to_string_lossy());
            ExitCode::FAILURE
        }
    }
}
