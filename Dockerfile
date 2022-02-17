FROM rust:1.58-slim-buster

WORKDIR /app

COPY . .

RUN apt-get update && apt-get install libpq-dev
RUN cargo build --release

EXPOSE 8080
CMD [ "./target/release/marche-server" ]
