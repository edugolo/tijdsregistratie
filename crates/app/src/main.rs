//! Time registration app: Dioxus fullstack entry point.

mod api;
mod client;
#[cfg(feature = "server")]
mod server;
mod shared;

fn main() {
    dioxus::launch(client::App);
}
