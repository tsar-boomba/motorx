# motorx-core

## Build custom Motorx binaries

```rs
use motorx_core::{Server, Config};

#[tokio::main]
async fn main() {
    let server = Server::new(Config { /* your config here */ });
    server.run().await.unwrap();
}
```
