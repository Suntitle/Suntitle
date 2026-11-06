use std::fs;
use std::path::PathBuf;

use control_api::api::ApiDoc;
use utoipa::OpenApi;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../specs/openapi.yaml");
    let mut yaml = ApiDoc::openapi().to_yaml()?;
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    fs::write(&output, yaml)?;
    println!("wrote {}", output.display());
    Ok(())
}
