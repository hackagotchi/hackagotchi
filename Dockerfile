FROM rustlang/rust:nightly

WORKDIR /usr/src/app

COPY . .

RUN cargo build --release

CMD ["./target/release/gotchi"]
