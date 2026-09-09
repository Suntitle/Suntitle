use std::path::PathBuf;

use actix_web::{App, HttpServer};
use clap::{Parser, Subcommand};
use utoipa_actix_web::AppExt;
use ws_proxy::kubernetes;

#[derive(Parser)]
#[command(about = "Suntitle control API and Kubernetes resource bootstrap")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    /// Optionally install CRDs before starting the existing PostgreSQL API.
    #[arg(long, env = "SUNTITLE_BOOTSTRAP_KUBECONFIG")]
    bootstrap_kubeconfig: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Print CRD YAML without connecting to Kubernetes or PostgreSQL.
    GenerateCrds,
    /// Apply namespace, CRDs and API/proxy RBAC; wait for established CRDs.
    BootstrapKubernetes {
        #[arg(long, env = "SUNTITLE_BOOTSTRAP_KUBECONFIG")]
        kubeconfig: PathBuf,
        #[arg(
            long,
            default_value = "suntitle",
            env = "SUNTITLE_KUBERNETES_NAMESPACE"
        )]
        namespace: String,
    },
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();
    match args.command {
        Some(Command::GenerateCrds) => {
            print!(
                "{}",
                kubernetes::resources::crd_yaml().map_err(std::io::Error::other)?
            );
            return Ok(());
        }
        Some(Command::BootstrapKubernetes {
            kubeconfig,
            namespace,
        }) => {
            let client = kubernetes::client_from_kubeconfig(&kubeconfig)
                .await
                .map_err(std::io::Error::other)?;
            kubernetes::bootstrap(client, &namespace)
                .await
                .map_err(std::io::Error::other)?;
            eprintln!("Kubernetes resources established in namespace {namespace}");
            return Ok(());
        }
        None => {}
    }
    if let Some(path) = args.bootstrap_kubeconfig {
        let client = kubernetes::client_from_kubeconfig(&path)
            .await
            .map_err(std::io::Error::other)?;
        kubernetes::apply_crds(client)
            .await
            .map_err(std::io::Error::other)?;
    }
    let url = ws_proxy::storage::postgres::database_url()?;
    let store: ws_proxy::storage::Store =
        std::sync::Arc::new(ws_proxy::storage::PostgresStore::connect(&url).await?);
    eprintln!("Suntitle REST API listening on 0.0.0.0:3031");
    HttpServer::new(move || {
        let store = store.clone();
        App::new()
            .into_utoipa_app()
            .configure(move |c| ws_proxy::api::configure_with_store(c, store))
            .into_app()
    })
    .bind(("0.0.0.0", 3031))?
    .run()
    .await
}
