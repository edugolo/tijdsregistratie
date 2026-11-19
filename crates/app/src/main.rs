//! Time registration app: Dioxus fullstack entry point.

mod api;
mod client;
// Several server helpers are used by later tasks (auth, repository).
#[cfg(feature = "server")]
#[allow(dead_code)]
mod server;
mod shared;

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(client::App);
}

#[cfg(feature = "server")]
fn main() {
    server::serve(client::App);
}
