FROM rust:latest as builder

WORKDIR /usr/src/pocketworlds
COPY . .
RUN cargo build --release

FROM debian:bookworm
COPY --from=builder /usr/src/pocketworlds/target/release/pocketworlds /usr/local/bin/pocketworlds
CMD ["pocketworlds"]