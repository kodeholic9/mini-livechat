// author: kodeholic (powered by Gemini)

pub mod config;
pub mod utils;
pub mod error;
pub mod core;

pub async fn run_server() {
    println!("[mini-livechat] Engine is ready (Port: {})", config::SERVER_UDP_PORT);
}