use std::path::PathBuf;

use actix_web::{App, HttpServer};
use clap::{Parser, Subcommand};
use suntitle_core::kubernetes;
use utoipa_actix_web::AppExt;

#[derive(Parser)]
#[command(about = "Suntitle control API and Kubernetes resource bootstrap")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(long, env = "SUNTITLE_API_KUBECONFIG")]
    kubeconfig: Option<PathBuf>,
    #[arg(
        long,
        default_value = "suntitle",
        env = "SUNTITLE_KUBERNETES_NAMESPACE"
    )]
    namespace: String,
    /// Optionally install CRDs before starting the API.
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
    let path = args.kubeconfig.ok_or_else(|| {
        std::io::Error::other("--kubeconfig or SUNTITLE_API_KUBECONFIG is required")
    })?;
    let client = kubernetes::client_from_kubeconfig(&path)
        .await
        .map_err(std::io::Error::other)?;
    let store = control_api::store::ResourceStore::new(client, &args.namespace);
    store.ready().await.map_err(std::io::Error::other)?;
    let database_url = std::env::var("DATABASE_URL").map_err(std::io::Error::other)?;
    let auth = control_api::api::auth::AuthState::from_database(&database_url)
        .await
        .map_err(std::io::Error::other)?;
    let auth = actix_web::web::Data::new(auth);
    eprintln!("Suntitle REST API listening on 0.0.0.0:3031");
    HttpServer::new(move || {
        let store = store.clone();
        App::new()
            .app_data(auth.clone())
            .into_utoipa_app()
            .configure(move |c| control_api::api::configure_with_store(c, store))
            .into_app()
    })
    .bind(("0.0.0.0", 3031))?
    .run()
    .await
}
