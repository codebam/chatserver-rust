use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::vec::Vec;

#[derive(Debug)]
struct ServerData {
    version: usize,
    clients: Mutex<Vec<Option<Client>>>,
    next_id: Mutex<usize>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Packet {
    number: usize,
    version: usize,
    source: String,
    dest: String,
    verb: String,
    data: String,
}

#[derive(Debug)]
struct Client {
    stream: Option<TcpStream>,
    username: String,
    id: usize,
    version: usize,
}

fn handle_client(mut stream: TcpStream, server: Arc<ServerData>) -> Result<(), std::io::Error> {
    let mut data = Vec::new();
    let mut reader = BufReader::new(stream);

    loop {
        data.clear();
        // clear our data buffer to read into

        let bytes_read = reader.read_until(b'\n', &mut data)?;
        // read until we hit a newline character
        if bytes_read == 0 {
            return Ok(());
        } // close the stream if there are no bytes left to read

        let packet: Packet = serde_json::from_slice(&data)?;
        if packet.version != server.version {
            return Ok(());
        } // close the stream if the packet version is different

        match packet.verb.as_ref() {
            "LOGN" => {
                let new_client = Client {
                    stream: Some(stream),
                    username: packet.source,
                    id: server.next_id.lock()?,
                    version: packet.version,
                };
            }
            "GDBY" => {}
            _ => Err(()),
        }
        println!("{:#?}", packet);
    }
}

fn main() -> Result<(), std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:59444")?;
    let server = Arc::new(ServerData {
        version: 1,
        clients: Mutex::new(vec![None]),
        next_id: Mutex::new(1),
    });
    println!("Server listening on port 59444");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let server = server.clone();
                println!("New connection: {}", stream.peer_addr()?);
                thread::spawn(move || {
                    handle_client(stream, server)
                    // connection succeeded
                });
            }
            Err(e) => {
                println!("Error: {}", e);
                return Err(e);
                // connection failed
            }
        }
    }
    // close the socket server
    drop(listener);
    Ok(())
}
