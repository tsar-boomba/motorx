//! Very simple http echo server used in benchmarks

use std::net::SocketAddr;

use http::Response;
use hyper::{server, service::service_fn};
use hyper_util::rt::tokio::{TokioIo, TokioExecutor};

#[tokio::main]
async fn main() {
    let addr: SocketAddr = std::env::args().collect::<Vec<String>>()[1]
        .parse()
        .unwrap();

    let socket = tokio::net::TcpListener::bind(addr).await.unwrap();
    println!("Echo server listening on http://{}", addr);

    loop {
        if let Ok((stream, _)) = socket.accept().await {
            let service =
                service_fn(|req| async { Ok::<_, hyper::Error>(Response::new(req.into_body())) });
            tokio::spawn(async move {
                if let Err(_) = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(TokioIo::new(stream), service)
                    .await
                {};
            });
        }
    }
}
