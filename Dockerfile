FROM rust:1.57.0:nightly

WORKDIR /app

COPY . .

RUN cargo build --release

EXPOSE 8080
CMD [ "./target/release/marche-server" ]
