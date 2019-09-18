use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::str;
use std::sync::{Arc, Mutex};
use std::thread;
use std::vec::Vec;

const VERSION: &str = "FC1";

fn handle_client(
    mut stream: TcpStream,
    clients: Arc<Mutex<Vec<Client>>>,
    id: Arc<Mutex<Vec<usize>>>, // stack to keep track of user ids
) {
    let mut data = [0 as u8; 256]; // using 256 byte buffer
    let client = ClientHandler {
        client_list: clients.clone(),
        socket_addr: stream.peer_addr().unwrap(),
    };
    'connection: while match stream.read(&mut data) {
        // close connection when the client disconnects
        Ok(0) => {
            println!("closing connection. client disconnected");
            break 'connection;
        }
        Ok(_) => {
            let mut words = str::from_utf8(&data).unwrap().split_whitespace();
            let command = words.next();
            if !clients.lock().unwrap().connected(client.socket_addr) {
                match command {
                    Some("LOGN") => {
                        let c = Client {
                            ip: client.socket_addr,
                            username: words.next().unwrap().to_string(),
                            version: words.next().unwrap().to_string(),
                        };
                        if VERSION == c.version {
                            println!("client authenticated: {:#?}", c);
                            clients.lock().unwrap().push(c);

                            let user_id: usize = match id.lock() {
                                Ok(mut id) => id.pop().unwrap(),
                                Err(_) => 999,
                                // default user id is 999
                            };

                            stream
                                .write(format!("SUCC {}\n", user_id).as_bytes())
                                .unwrap();
                        } else {
                            println!("closing connection. client has different version");
                            break 'connection;
                        } // only accept client if they're using the same version as us
                    } // client authentication
                    Some(_) => {
                        println!("closing connection. invalid command");
                        break 'connection;
                    }
                    // invalid command
                    None => {
                        println!("closing connection. no more data2");
                        break 'connection;
                    } // no more data
                }
            } // don't allow clients to authenticate more than once

            if clients.lock().unwrap().connected(client.socket_addr) {
                match command {
                    Some("") => {} // auth new client
                    Some(_) => {}
                    None => {}
                }
            } else {
                println!("closing connection. client didn't login");
                break 'connection;
            } // only let logged in clients issue commands

            println!("{}", str::from_utf8(&data).unwrap());
            println!("clients: {:#?}", clients);
            // debug
            true
        }
        Err(_) => {
            println!(
                "An error occurred, terminating connection with {}",
                client.socket_addr
            );
            stream.shutdown(Shutdown::Both).unwrap();
            false
        }
    } {}
}

#[derive(Debug)]
struct Client {
    ip: SocketAddr,
    username: String,
    version: String,
}

struct ClientHandler {
    client_list: Arc<Mutex<Vec<Client>>>,
    socket_addr: SocketAddr,
}

impl Drop for ClientHandler {
    fn drop(&mut self) {
        match self
            .client_list
            .lock()
            .unwrap()
            .iter()
            .position(|client_list| client_list.ip == self.socket_addr)
        {
            Some(i) => {
                self.client_list.lock().unwrap().remove(i);
                ()
            }
            None => {} // if the client doesn't exist, do nothing
        }
        println!("client dropped.");
    }
} // automatically drop connections when clients go out of scope

impl PartialEq for Client {
    fn eq(&self, other: &Self) -> bool {
        self.ip == other.ip
    }
}

trait ClientMethods {
    fn connected(&self, ip: SocketAddr) -> bool;
}

impl ClientMethods for Vec<Client> {
    fn connected(&self, ip: SocketAddr) -> bool {
        for c in self.iter() {
            if c.ip == ip {
                return true;
            } // client found
        }
        false
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:3333").unwrap();
    let clients = Arc::new(Mutex::new(vec![Client {
        ip: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0),
        username: "root".to_string(),
        version: VERSION.to_string(),
    }]));
    let max_id = 100;
    let user_id = Arc::new(Mutex::new((1..max_id).rev().collect()));
    // accept connections and process them, spawning a new thread for each one
    println!("Server listening on port 3333");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let clients = clients.clone();
                let user_id = user_id.clone();
                println!("New connection: {}", stream.peer_addr().unwrap());
                thread::spawn(move || {
                    // connection succeeded
                    handle_client(stream, clients, user_id)
                });
            }
            Err(e) => {
                println!("Error: {}", e);
                /* connection failed */
            }
        }
    }
    // close the socket server
    drop(listener);
}
