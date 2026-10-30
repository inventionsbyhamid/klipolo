//! Frame transport: rendered RGBA frames go to the webview over a loopback websocket, newest frame first.

pub mod buffer;
pub mod server;
pub mod trailer;
