use std::env;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc;

fn append(path: &Path, event: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(format!("{event}\n").as_bytes())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let log_path =
        env::var_os("FAKE_NODE_LOG").ok_or("FAKE_NODE_LOG must point to the E2E process log")?;
    let log_path = std::path::PathBuf::from(log_path);
    let pid = std::process::id();
    let args: Vec<_> = env::args().skip(1).collect();
    append(
        &log_path,
        &format!("START\t{pid}\t{}", serde_json::to_string(&args)?),
    )?;

    let (shutdown_tx, shutdown_rx) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = shutdown_tx.send(());
    })?;
    shutdown_rx.recv()?;
    append(&log_path, &format!("STOP\t{pid}"))?;
    Ok(())
}
