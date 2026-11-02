//! Native fixture records launch evidence, then exits on a finish file or timeout.
use std::{
    env, fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

/// Record the actual arguments and installed executable without interpreting shell syntax.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let value = |flag: &str| -> Result<&str, Box<dyn std::error::Error>> {
        args.windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
            .ok_or_else(|| format!("missing {flag}").into())
    };
    let report = serde_json::json!({
        "args": args, "pid": std::process::id(),
        "executable": env::current_exe()?, "cwd": env::current_dir()?
    });
    fs::write(value("--report")?, serde_json::to_vec(&report)?)?;
    let deadline = Instant::now() + Duration::from_secs(120);
    while !Path::new(value("--finish")?).exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
