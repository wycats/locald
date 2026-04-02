use futures::stream::StreamExt;
use std::collections::HashMap;
use std::ffi::CString;
use xpc_connection::{Message, XpcClient};

#[tokio::main]
async fn main() {
    let name = CString::new("com.locald.spike").unwrap();
    println!("Connecting to {:?}", name);

    let mut client = XpcClient::connect(&name);

    // Send a dictionary message
    let mut msg = HashMap::new();
    msg.insert(
        CString::new("command").unwrap(),
        Message::String(CString::new("ping").unwrap()),
    );
    msg.insert(CString::new("value").unwrap(), Message::Int64(42));
    client.send_message(Message::Dictionary(msg));

    // Wait for response
    match client.next().await {
        Some(response) => println!("Response: {:?}", response),
        None => println!("No response (connection closed)"),
    }

    println!("Done");
}
