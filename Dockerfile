FROM rustlang/rust:nightly

WORKDIR /app

COPY . .

RUN cargo build --release

EXPOSE 8080
CMD [ "./target/release/marche-server" ]
