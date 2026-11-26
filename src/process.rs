mod invocation;
use invocation::{IGNORE_SIGPIPE, INVOCATION};
pub use invocation::{Invocation, save_invocation, wrapper_main};

use crate::artifact::Artifact;
use crate::files::{GeneratedFile, InputFiles};
#[cfg(feature = "publish")]
use crate::output::PendingOutput;
use crate::scratch::ScratchDirectory;
use libc::{SIGCHLD, SIGPIPE};
use rustix::io::FdFlags;
use rustix::process::{Pid, Signal, kill_process, kill_process_group};
use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
#[cfg(feature = "publish")]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::sync::atomic::Ordering;

pub type Argument = Vec<u8>;
pub type Arguments = Vec<Argument>;

/// Replacement arguments and the files they refer to.
pub struct PreparedArgs {
    args: Arguments,
    inputs: InputFiles,
}

impl PreparedArgs {
    pub fn new(args: Arguments, inputs: InputFiles) -> Self {
        Self { args, inputs }
    }
}

pub struct Tool {
    name: Cow<'static, OsStr>,
    pub args: Arguments,
    launcher_options: Arguments,
    argv0: OsString,
    vendor: PathBuf,
    inputs: InputFiles,
}

impl Tool {
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Private options supplied before the installed path by a shared launcher.
    /// These bytes are never forwarded to the wrapped program.
    pub fn launcher_options(&self) -> &[Argument] {
        &self.launcher_options
    }

    /// Use staged arguments when available and retain their files for subsequent launches.
    pub fn with_prepared_args(mut self, prepared: Option<PreparedArgs>) -> Self {
        if let Some(prepared) = prepared {
            self.args = prepared.args;
            self.inputs.append(prepared.inputs);
        }
        self
    }

    /// Prepare a command with inherited standard I/O and the staged files
    /// required by the arguments. Descriptors closed at wrapper startup stay
    /// closed in the child, even if the command is later configured to redirect them.
    pub fn command(&self, args: &[Argument]) -> Command {
        self.command_from(args.iter().map(Vec::as_slice))
    }

    /// Configure a command from borrowed argument bytes. This accepts
    /// composed iterators without copying the arguments into a temporary list.
    pub fn command_from<'a>(&self, args: impl IntoIterator<Item = &'a [u8]>) -> Command {
        let mut command = self.vendor_command();
        command.args(args.into_iter().map(OsStr::from_bytes));
        command
    }

    fn vendor_command(&self) -> Command {
        let mut command = Command::new(&self.vendor);
        command.arg0(&self.argv0);
        let state = INVOCATION.load(Ordering::Relaxed);
        if state != 0 || self.inputs.len() != 0 {
            let inputs = self.inputs.clone();
            // SAFETY: close, signal, and fcntl are async-signal-safe. The captured
            // files stay open for the lifetime of the command.
            unsafe {
                command.pre_exec(move || {
                    for fd in 0..=libc::STDERR_FILENO {
                        if state & (1 << fd) != 0 {
                            libc::close(fd);
                        }
                    }
                    // Command resets SIGPIPE before calling this hook. exec
                    // normally preserves an inherited SIG_IGN disposition.
                    if state & IGNORE_SIGPIPE != 0
                        && libc::signal(SIGPIPE, libc::SIG_IGN) == libc::SIG_ERR
                    {
                        return Err(io::Error::last_os_error());
                    }
                    inputs.set_flags(FdFlags::empty())
                });
            }
        }
        command
    }

    pub fn use_installed_name(&mut self) {
        self.argv0 = self.vendor.with_file_name(self.name()).into_os_string();
    }

    fn launch_failed(&self, error: io::Error) -> i32 {
        eprintln!(
            "{} wrapper: error: exec {}: {error}",
            self.name.to_string_lossy(),
            self.vendor.display()
        );
        127
    }

    /// Replace the wrapper with the tool using its current arguments.
    pub fn forward(&self) -> i32 {
        self.exec(self.command(&self.args))
    }

    pub fn exec(&self, mut command: Command) -> i32 {
        let error = command.exec();
        // A failed exec may have cleared CLOEXEC in this process.
        let _ = self.inputs.set_flags(FdFlags::CLOEXEC);
        self.launch_failed(error)
    }

    /// Run with the current arguments and propagate the tool's exit code or terminating signal.
    pub fn supervise(&self) -> io::Result<i32> {
        self.run(self.command(&self.args), None).map(exit_status)
    }

    /// Run and reap a child while forwarding cancellation signals.
    ///
    /// Call this from a single-threaded wrapper that owns child reaping. It
    /// temporarily changes the process's SIGCHLD disposition and waits for
    /// blocked signals on the calling thread. The wrapped program may start
    /// its own threads and subprocesses. This method waits for the direct child.
    pub fn run(&self, mut command: Command, lease: Option<&File>) -> io::Result<ExitStatus> {
        let mask = SignalMask::block()?;
        let foreground = in_foreground();
        if !foreground {
            command.process_group(0);
        }
        let original = mask.original;
        let original_child = mask.original_child;
        let parent = rustix::process::getpid();
        let lease = lease.map(AsRawFd::as_raw_fd);
        // SAFETY: the closure uses only async-signal-safe system calls and
        // constructs errors from errno. Captured descriptors stay open until spawn returns.
        unsafe {
            command.pre_exec(move || {
                rustix::process::set_parent_process_death_signal(Some(Signal::KILL))?;
                if rustix::process::getppid() != Some(parent) {
                    libc::_exit(127);
                }
                if let Some(lease) = lease {
                    // Keep the open file description alive without using stdio.
                    check(libc::fcntl(lease, libc::F_DUPFD, libc::STDERR_FILENO + 1))?;
                }
                restore_signals(&original, &original_child)
            });
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Ok(ExitStatus::from_raw(self.launch_failed(error) << 8));
            }
        };
        let pid = Pid::from_child(&child);
        let forward = if foreground {
            kill_process
        } else {
            kill_process_group
        };
        loop {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
            // SAFETY: the signal set is blocked and sigwaitinfo initializes info on success.
            let signal = unsafe { libc::sigwaitinfo(&mask.watched, info.as_mut_ptr()) };
            if signal < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            // SAFETY: sigwaitinfo succeeded, so it initialized the signal information.
            let origin = unsafe { info.assume_init().si_code };
            if let Some(signal) = Signal::from_named_raw(signal)
                && signal != Signal::CHILD
                && (!foreground || origin != libc::SI_KERNEL)
            {
                let _ = forward(pid, signal);
            }
        }
    }

    /// Run a command and call `complete` if it exits successfully. Release the
    /// optional scratch directory before completion or propagation of the exit
    /// status. On failure, drop the callback before propagating the status so
    /// its captured resources can be released.
    pub fn run_then(
        &self,
        command: Command,
        scratch: Option<ScratchDirectory>,
        complete: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<i32> {
        let status = self.run(command, scratch.as_ref().and_then(ScratchDirectory::lease))?;
        drop(scratch);
        if status.success() {
            complete()?;
        } else {
            drop(complete);
        }
        Ok(exit_status(status))
    }

    /// Run a producer against a private output path, then publish its edited file.
    #[cfg(feature = "publish")]
    pub fn publish_output(
        &self,
        destination: &Path,
        command: impl FnOnce(&Path) -> io::Result<Command>,
        edit: impl FnOnce(&mut Artifact<'_>) -> io::Result<()>,
    ) -> io::Result<i32> {
        let pending = PendingOutput::new(destination)?;
        self.run_then(command(pending.path())?, None, || pending.publish(edit))
    }

    /// Edit a freshly generated output after a successful run. Relative
    /// paths are interpreted in the command's working directory.
    pub fn edit_output(
        &self,
        command: Command,
        output: PathBuf,
        edit: impl FnOnce(&mut Artifact<'_>) -> io::Result<()>,
    ) -> io::Result<i32> {
        let path = match command.get_current_dir() {
            Some(directory) if output.is_relative() && !output.as_os_str().is_empty() => {
                directory.join(output)
            }
            _ => output,
        };
        let output = GeneratedFile::new(path)?;
        self.run_then(command, None, || {
            output.edit(&self.name.to_string_lossy(), edit)
        })
    }
}

struct SignalMask {
    watched: libc::sigset_t,
    original: libc::sigset_t,
    original_child: libc::sigaction,
}

impl SignalMask {
    fn block() -> io::Result<Self> {
        // SAFETY: All-zero sigset_t storage is valid for both libc targets.
        // Zero original before the kernel writes its (possibly smaller) mask.
        unsafe {
            let mut watched = std::mem::zeroed();
            let mut original = std::mem::zeroed();
            libc::sigemptyset(&mut watched);
            for signal in [
                Signal::HUP,
                Signal::INT,
                Signal::QUIT,
                Signal::TERM,
                Signal::USR1,
                Signal::USR2,
                Signal::CHILD,
            ] {
                libc::sigaddset(&mut watched, signal.as_raw());
            }
            check(libc::sigprocmask(libc::SIG_BLOCK, &watched, &mut original))?;
            // SIG_IGN or SA_NOCLDWAIT inherited by the supervisor would
            // cause the kernel to reap the vendor before we can wait for it.
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut action.sa_mask);
            let mut original_child = std::mem::zeroed();
            if let Err(error) = check(libc::sigaction(SIGCHLD, &action, &mut original_child)) {
                libc::sigprocmask(libc::SIG_SETMASK, &original, std::ptr::null_mut());
                return Err(error);
            }
            Ok(Self {
                watched,
                original,
                original_child,
            })
        }
    }
}

impl Drop for SignalMask {
    fn drop(&mut self) {
        let _ = restore_signals(&self.original, &self.original_child);
    }
}

fn restore_signals(mask: &libc::sigset_t, child: &libc::sigaction) -> io::Result<()> {
    // SAFETY: SignalMask saved both values before changing them.
    unsafe {
        let action = check(libc::sigaction(SIGCHLD, child, std::ptr::null_mut()));
        let mask = check(libc::sigprocmask(
            libc::SIG_SETMASK,
            mask,
            std::ptr::null_mut(),
        ));
        action.and(mask)
    }
}

fn check(result: libc::c_int) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn in_foreground() -> bool {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/tty")
        .is_ok_and(|tty| rustix::termios::tcgetpgrp(tty) == Ok(rustix::process::getpgrp()))
}

/// Return the exit code or reset, unblock, and re-raise the terminating
/// signal in the wrapper process. Run this after releasing resources.
pub fn exit_status(status: ExitStatus) -> i32 {
    if let Some(signal) = status.signal() {
        // SAFETY: status supplies a valid signal number.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            let mut only = std::mem::zeroed();
            libc::sigemptyset(&mut only);
            libc::sigaddset(&mut only, signal);
            libc::sigprocmask(libc::SIG_UNBLOCK, &only, std::ptr::null_mut());
            libc::raise(signal);
        }
        return 128 + signal;
    }
    status.code().unwrap_or(1)
}

#[cfg(test)]
mod tests;
