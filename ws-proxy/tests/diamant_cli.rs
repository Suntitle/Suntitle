#![cfg(not(feature = "gui"))]

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[tokio::test]
async fn default_build_starts_headless_with_or_without_cli_flag() {
    for args in [vec![], vec!["--cli"]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_diamant"))
            .args(args)
            .args([
                "--proxy-url",
                "invalid-url",
                "--host-name",
                &uuid::Uuid::new_v4().to_string(),
            ])
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let output = tokio::time::timeout(Duration::from_secs(10), async {
            let mut output = String::new();
            while let Some(line) = lines.next_line().await.unwrap() {
                output.push_str(&line);
                output.push('\n');
                if output.contains("invalid proxy URL") {
                    break;
                }
            }
            output
        })
        .await;
        child.kill().await.unwrap();
        let output = output.unwrap();
        assert!(output.contains("registration disconnected:"), "{output}");
        assert!(output.contains("invalid proxy URL"), "{output}");
    }
}

#[test]
fn default_build_runs_embedded_uv() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_diamant"))
        .args(["uv", "--version"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8(output.stdout).unwrap().starts_with("uv "));
}
