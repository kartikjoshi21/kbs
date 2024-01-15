use std::{net::SocketAddr, path::Path, sync::Arc};

use actix_web::{web, App, HttpServer};
use thiserror::Error;
use anyhow::Result;
use attestation_service::{config::Config, AttestationService, ServiceError,  config::ConfigError};
use clap::{arg, command, Parser};
use log::info;
use openssl::{
    pkey::PKey,
    ssl::{SslAcceptor, SslMethod},
};
use strum::{AsRefStr, EnumString};
use tokio::sync::RwLock;

use crate::restful::{attestation, set_policy};

mod restful;

/// RESTful-AS command-line arguments.
#[derive(Debug, Parser)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Path to a CoCo-AS config file.
    #[arg(short, long)]
    pub config_file: Option<String>,

    /// Socket addresses (IP:port) to listen on, e.g. 127.0.0.1:8080.
    #[arg(short, long)]
    pub socket: SocketAddr,

    /// Path to the public key cert for HTTPS. Both public key cert and
    /// private key are provided then HTTPS will be enabled.
    #[arg(short, long)]
    pub https_pubkey_cert: Option<String>,

    /// Path to the private key for HTTPS. Both public key cert and
    /// private key are provided then HTTPS will be enabled.
    #[arg(short, long)]
    pub https_prikey: Option<String>,
}

#[derive(EnumString, AsRefStr)]
#[strum(serialize_all = "lowercase")]
enum WebApi {
    #[strum(serialize = "/attestation")]
    Attestation,

    #[strum(serialize = "/policy")]
    Policy,
}

#[derive(Error, Debug)]
pub enum RestfulError {
    #[error("Creating service failed: {0}: {0}")]
    Service(#[from] ServiceError),
    #[error("Failed to read AS config file: {0}")]
    Config(#[from] ConfigError),
    #[error("Openssl errorstack: {0}")]
    Openssl(#[from]  openssl::error::ErrorStack),
    #[error("io error")]
    IO(#[from] std::io::Error),
    #[error("Failed to start server: {0}")]
    StartServer(String),
}

#[actix_web::main]
async fn main() -> Result<(), RestfulError> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let cli = Cli::parse();

    let config = match cli.config_file {
        Some(path) => {
            info!("Using config file {path}");
            Config::try_from(Path::new(&path))?
        }
        None => {
            info!("No confile path provided, use default one.");
            Config::default()
        }
    };

    let attestation_service = AttestationService::new(config).await?;

    let attestation_service = web::Data::new(Arc::new(RwLock::new(attestation_service)));
    let server = HttpServer::new(move || {
        App::new()
            .service(web::resource(WebApi::Attestation.as_ref()).route(web::post().to(attestation)))
            .service(web::resource(WebApi::Policy.as_ref()).route(web::post().to(set_policy)))
            .app_data(web::Data::clone(&attestation_service))
    });

    let server = match (cli.https_prikey, cli.https_pubkey_cert) {
        (Some(prikey), Some(pubkey_cert)) => {
            let mut builder = SslAcceptor::mozilla_modern(SslMethod::tls())?;

            let prikey = tokio::fs::read(prikey)
                .await?;
            let prikey =
                PKey::private_key_from_pem(&prikey)?;

            builder
                .set_private_key(&prikey)?;
            builder
                .set_certificate_chain_file(pubkey_cert)?;
            log::info!("starting HTTPS server at https://{}", cli.socket);
            server.bind_openssl(cli.socket, builder)?
            .run()
        }
        _ => {
            log::info!("starting HTTP server at http://{}", cli.socket);
            server
                .bind((cli.socket.ip().to_string(), cli.socket.port()))?
                .run()
        }
    };

    server.await.map_err(|e| RestfulError::StartServer(e.to_string()))?;

    Ok(())
}
