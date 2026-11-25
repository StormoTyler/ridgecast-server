# builds the RidgeCast server into a small image. the environment variables
# (see .env.example) come from the host when the container starts, never from
# the image: .dockerignore keeps .env out of the build

# the same Rust as CI, on the same Debian release as the runtime image below,
# so the binary links against the glibc it runs with
FROM rust:1.97.0-slim-trixie AS build
WORKDIR /app
COPY . .
# the cache mounts keep downloaded crates and compiled dependencies between
# builds. target/ is a cache mount, so the binary is copied out of it.
# --locked builds exactly the versions in Cargo.lock
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked \
    && cp target/release/ridgecast /usr/local/bin/ridgecast

FROM debian:trixie-slim
# reqwest checks TLS certificates against the system's certificate store, so
# requests to NWS, NAC, and Telnyx fail without it
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# never run as root
RUN useradd --system --no-create-home --shell /usr/sbin/nologin ridgecast
COPY --from=build /usr/local/bin/ridgecast /usr/local/bin/ridgecast
USER ridgecast
# a container is reached from outside it, so listen on every interface.
# PORT is left at its default, 5001
ENV HOST=0.0.0.0
EXPOSE 5001
CMD ["/usr/local/bin/ridgecast"]
