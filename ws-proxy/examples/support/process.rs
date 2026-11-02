//! Application guards stop and reap children before test storage is removed.
use std::{
    env, io,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, time::sleep};

/// Application child owned by one test. Drop attempts graceful shutdown before kill.
pub struct ChildProcess {
    /// Application name included in failure diagnostics.
    name: &'static str,
    /// Child MUST be reaped before this guard releases its resources.
    child: Child,
}

impl ChildProcess {
    /// Start an application and inherit its diagnostics in the test runner.
    pub fn spawn(name: &'static str, command: &mut Command) -> io::Result<Self> {
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        Ok(Self { name, child })
    }

    /// Fail immediately if an application exits while its services are required.
    pub fn ensure_running(&mut self) -> io::Result<()> {
        if let Some(status) = self.child.try_wait()? {
            return Err(io::Error::other(format!(
                "{} exited before the E2E check completed: {status}",
                self.name
            )));
        }
        Ok(())
    }

    /// Request graceful Unix shutdown so Diamant can reap its supervised processes.
    pub fn terminate(&mut self) -> io::Result<()> {
        if self.child.try_wait()?.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        {
            let pid = i32::try_from(self.child.id())
                .map_err(|_| io::Error::other("child process ID exceeds i32"))?;
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            )
            .map_err(io::Error::other)
        }
        #[cfg(not(unix))]
        self.child.kill()
    }

    /// Wait for successful termination with a bounded deadline.
    pub async fn wait_for_exit(&mut self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other(format!(
                        "{} exited unsuccessfully: {status}",
                        self.name
                    )))
                };
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for {} to stop", self.name),
                ));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for ChildProcess {
    /// Allow graceful cleanup, then force termination and reap the child.
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.terminate();
            let deadline = Instant::now() + Duration::from_secs(7);
            while Instant::now() < deadline {
                if self.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Locate prepared application binaries beside the currently running Cargo example.
pub fn binary_directory() -> io::Result<PathBuf> {
    env::current_exe()?
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| io::Error::other("could not determine Cargo target directory"))
}

/// Resolve a prepared binary using the platform executable suffix.
pub fn binary_path(directory: &Path, name: &str) -> PathBuf {
    directory.join(format!("{name}{}", env::consts::EXE_SUFFIX))
}

/// Wait for both services while checking all required application children.
pub async fn wait_for_services(processes: &mut [&mut ChildProcess]) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        for process in processes.iter_mut() {
            process.ensure_running()?;
        }
        if TcpStream::connect("127.0.0.1:3030").await.is_ok()
            && TcpStream::connect("127.0.0.1:3031").await.is_ok()
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for proxy port 3030 and API port 3031",
            ));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// Pin the real local interpreter so embedded UV can run dependency-free fixtures offline.
pub fn fixture_python() -> io::Result<PathBuf> {
    let output = Command::new("/usr/bin/python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err(io::Error::other(
            "could not locate the fixture Python interpreter",
        ));
    }
    let path = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    Ok(PathBuf::from(path.trim()))
}
