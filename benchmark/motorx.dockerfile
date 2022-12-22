FROM rust:1.66-slim-bullseye as echo
WORKDIR /app
COPY ./echo-server .
RUN cargo build --release

FROM rust:1.66-slim-bullseye as builder
WORKDIR /app

# im too lazy to do this efficiently
COPY ./motorx-core ./motorx-core
COPY ./src ./src
COPY ./Cargo.toml ./Cargo.toml
RUN cargo build --release -p motorx

FROM debian:bullseye-slim
COPY --from=echo /app/target/release/echo-server .
COPY --from=builder /app/target/release/motorx .
COPY ./benchmark/motorx.json motorx.json
COPY ./benchmark/with_echo.sh with_echo.sh
RUN chmod +x with_echo.sh
EXPOSE 80
CMD ["./with_echo.sh", "./motorx"]
