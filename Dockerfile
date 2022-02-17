FROM rust:1.58-slim-buster

WORKDIR /app

COPY . .

RUN cargo build --release

EXPOSE 8080
CMD [ "./target/release/marche-server" ]
