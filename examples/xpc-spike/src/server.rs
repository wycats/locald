use futures::stream::StreamExt;
use std::ffi::CString;
use xpc_connection::{Message, XpcClient, XpcListener};

async fn handle_client(mut client: XpcClient) {
    println!("New connection (audit_token: {:?})", client.audit_token());

    loop {
        match client.next().await {
            None => break,
            Some(Message::Error(e)) => {
                println!("Error: {:?}", e);
                break;
            }
            Some(m) => {
                println!("Received: {:?}", m);
                client.send_message(m);
            }
        }
    }

    println!("Connection closed");
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let name = CString::new("com.locald.spike").unwrap();
    println!("Listening on {:?}", name);

    let mut listener = XpcListener::listen(&name);

    while let Some(client) = listener.next().await {
        tokio::spawn(handle_client(client));
    }
}
