FROM rust:1.57.0

WORKDIR /app

COPY . .

RUN cargo build --release

ENTRYPOINT ["./target/release/marche-server"]

EXPOSE 8080
