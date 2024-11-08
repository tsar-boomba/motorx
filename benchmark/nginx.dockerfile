FROM rust:1.82-slim-bullseye AS echo
WORKDIR /app
COPY ./echo-server .
RUN cargo build --release

# For NGINX in container
# Base on offical NGINX Alpine img
FROM nginx:stable

# Remove any existing config files
RUN rm /etc/nginx/conf.d/*

# Copy config files
# *.conf files in conf.d/ dir get included in main config 
COPY ./benchmark/default.conf /etc/nginx/conf.d/

# Expose the listening port
EXPOSE 80

COPY ./benchmark/with_echo.sh with_echo.sh
RUN chmod +x with_echo.sh

COPY --from=echo /app/target/release/echo-server .

# Launch NGINX
ENTRYPOINT ["./with_echo.sh", "nginx", "-g", "daemon off;"]
