use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::str;
use std::sync::{Arc, Mutex};
use std::thread;
use std::vec::Vec;

const VERSION: &str = "FC1";

fn client_connected(ip: SocketAddr, clients: &Arc<Mutex<Vec<Client>>>) -> bool {
    for c in clients.lock().unwrap().iter() {
        if c.ip == ip {
            println!("client found");
            return true;
        }
    }
    false
}

fn handle_client(
    mut stream: TcpStream,
    clients: Arc<Mutex<Vec<Client>>>,
    id: Arc<Mutex<Vec<usize>>>, // stack to keep track of user ids
) {
    let mut data = [0 as u8; 256]; // using 256 byte buffer
    'connection: while match stream.read(&mut data) {
        // close connection when the client disconnects
        Ok(0) => {
            println!("closing connection. client disconnected");
            break 'connection;
        }
        Ok(_) => {
            let mut words = str::from_utf8(&data).unwrap().split_whitespace();
            let command = words.next();
            match command {
                Some("LOGN") => {
                    let c = Client {
                        ip: stream.peer_addr().unwrap(),
                        username: words.next().unwrap().to_string(),
                    };
                    if VERSION == words.next().unwrap().to_string() {
                        println!("client authenticated: {:#?}", c);
                        clients.lock().unwrap().push(c);

                        let user_id: usize = match id.lock() {
                            Ok(mut id) => {
                                let t = id.len();
                                let y = id[t - 1];
                                id.truncate(t - 1);
                                y
                            }
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

            if client_connected(stream.peer_addr().unwrap(), &clients) {
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
                stream.peer_addr().unwrap()
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
}

fn create_user_ids(max_id: usize) -> Vec<usize> {
    let mut v = Vec::with_capacity(max_id);
    for i in (0..max_id).rev() {
        v.push(i);
    }
    return v;
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:3333").unwrap();
    let clients = Arc::new(Mutex::new(vec![Client {
        ip: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0),
        username: "root".to_string(),
    }]));
    let max_id = 100;
    let user_id = Arc::new(Mutex::new(create_user_ids(max_id)));
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
