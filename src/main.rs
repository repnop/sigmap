use nix::{
    sys::{pthread::pthread_kill, signal::Signal},
    unistd::Pid,
};
use std::{
    ffi::OsString,
    os::unix::thread::JoinHandleExt,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
};

/// Map a user-provided signal to a different signal to send to the child process
#[derive(argh::FromArgs)]
struct Arguments {
    #[argh(
        option,
        description = "signal value to receive from the user or another application (e.g. SIGINT, SIGTERM, ...)"
    )]
    from: Signal,
    #[argh(
        option,
        description = "signal value to send to the child process (e.g. SIGINT, SIGTERM, ...)"
    )]
    to: Signal,
    #[argh(positional, greedy)]
    command: Vec<OsString>,
}

fn main() {
    let Arguments { from, to, command } = argh::from_env();

    let mut program = command.into_iter();

    let mut child = match std::process::Command::new(program.next().unwrap())
        .args(program)
        .stdout(std::process::Stdio::piped())
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("Failed to spawn child process: {e}");
            std::process::exit(1);
        }
    };

    let mut stdout = child.stdout.take().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();

    let flag = Arc::new(AtomicBool::new(false));
    let flag2 = Arc::clone(&flag);

    let exit_status = Arc::new(AtomicI32::new(0));
    let exit_status2 = Arc::clone(&exit_status);

    let internal_signal = match from {
        Signal::SIGABRT => Signal::SIGINT,
        _ => Signal::SIGABRT,
    };

    let stdin_thread = std::thread::spawn(move || {
        signal_hook::flag::register(from as i32, flag2).unwrap();

        let _ = unsafe {
            signal_hook::low_level::register(internal_signal as i32, move || {
                libc::exit(exit_status2.load(Ordering::SeqCst))
            })
        };

        let mut our_stdin = std::io::stdin();
        let _ = std::io::copy(&mut our_stdin, &mut stdin);
    });

    let stdout_thread = std::thread::spawn(move || {
        let mut our_stdout = std::io::stdout();
        let _ = std::io::copy(&mut stdout, &mut our_stdout);
    });

    let stderr_thread = std::thread::spawn(move || {
        let mut our_stderr = std::io::stderr();
        let _ = std::io::copy(&mut stderr, &mut our_stderr);
    });

    let pid = Pid::from_raw(child.id() as i32);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));

        if flag.load(Ordering::Acquire) {
            flag.store(false, Ordering::Release);
            let _ = nix::sys::signal::kill(pid, to);
        }

        if let Ok(Some(status)) = child.try_wait() {
            exit_status.store(status.code().unwrap_or(0), Ordering::SeqCst);
            let _ = pthread_kill(stdin_thread.into_pthread_t(), internal_signal);
            let _ = pthread_kill(stdout_thread.into_pthread_t(), Signal::SIGKILL);
            let _ = pthread_kill(stderr_thread.into_pthread_t(), Signal::SIGKILL);
            break;
        }
    }
}
