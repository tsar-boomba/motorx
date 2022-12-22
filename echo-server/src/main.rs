//! Very simple http echo server used in benchmarks

use std::net::SocketAddr;

use http::Response;
use hyper::{server, service::service_fn};

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
                if let Err(_) = server::conn::http1::Builder::new()
                    .http1_preserve_header_case(true)
                    .http1_title_case_headers(true)
                    .http1_keep_alive(true)
                    .serve_connection(stream, service)
                    .with_upgrades()
                    .await
                {};
            });
        }
    }
}
