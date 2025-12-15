use actix_web::{App, HttpServer, Responder, get};

#[get("/")]
async fn hello() -> impl Responder {
    "Hello from Rust!"
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let port: u16 = port.parse().unwrap();
    println!("Starting server on port {}", port);
    HttpServer::new(|| App::new().service(hello))
        .bind(("0.0.0.0", port))?
        .run()
        .await
}
