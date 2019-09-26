use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::str;
use std::sync::{Arc, Mutex};
use std::thread;
use std::vec::Vec;

const VERSION: &str = "FC1";

fn data_parser(data: String) -> (String, String) {
    let mut words = data.split_whitespace();
    let command = words.next().unwrap().to_string();
    let mut chars = data.chars();
    for character in chars.by_ref() {
        if character.is_whitespace() {
            break;
        }
    }
    let options = chars.as_str().to_string();
    return (command, options);
}

fn send_parser(options: String) -> (String, String) {
    let mut words = options.split(":");
    let userid = words.next().unwrap().to_string();
    let mut chars = options.chars();
    for character in chars.by_ref() {
        if character == ':' {
            break;
        }
    }
    let message = chars.as_str().to_string();
    return (userid, message);
}

fn send_message(
    client: &ClientHandler,
    userid: String,
    message: String,
    clients: Arc<Mutex<Vec<Client>>>,
) -> Result<(), ()> {
    let clients_lock = clients.lock().unwrap();
    let from = clients_lock.get_client_by_ip(client.socket_addr).unwrap();
    let to = match clients_lock.get_client_by_userid(userid.clone()) {
        Some(to) => Some(to),
        None => match clients_lock.get_client_by_username(userid.clone()) {
            Some(to) => Some(to),
            None => None,
        },
    };
    match to {
        Some(to) => {
            // println!("{:#?}", message);
            let send = format!("RECV {}:{}", from.id, message);
            if to.id == 0 {
                for c in clients_lock.iter() {
                    match c.stream.try_clone().unwrap().write(send.as_bytes()) {
                        Ok(_) => {} // do nothing
                        Err(_) => println!("error, message failed to send to {:#?}", c),
                    }
                }
            }
            match to.stream.try_clone().unwrap().write(send.as_bytes()) {
                Ok(_) => {}
                Err(_) => {}
            }
            Ok(())
        }
        None => Err(()),
    }
}

fn whoo(mut stream: TcpStream, clients: Arc<Mutex<Vec<Client>>>) -> Result<(), ()> {
    let client_lock = clients.lock().unwrap();
    for c in client_lock.iter() {
        let send = format!("CONN {}:{}\n", c.id, c.username);
        let _ = stream.write(send.as_bytes());
    }
    Err(())
}

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
            let (command, options_string) = data_parser(str::from_utf8(&data).unwrap().to_string());
            let command = &*command;
            // deref our strings

            let mut words = options_string.split_whitespace();

            let connected = clients.lock().unwrap().connected(client.socket_addr);
            if connected {
                match command {
                    "GDBY" => break 'connection, // graceful disconnect
                    "SEND" => {
                        let (userid, message) = send_parser(options_string);
                        let _ = send_message(&client, userid, message, clients.clone());
                    }
                    "WHOO" => {
                        let _ = whoo(stream.try_clone().unwrap(), clients.clone());
                    }
                    _ => break 'connection,
                }
            } else {
                match command {
                    "LOGN" => {
                        let mut c = Client {
                            stream: stream.try_clone().unwrap(),
                            ip: client.socket_addr,
                            username: words.next().unwrap().to_string(),
                            id: 999,
                            version: words.next().unwrap().to_string(),
                        };
                        if VERSION == c.version {
                            let user_id = match id.lock() {
                                Ok(mut id) => id.pop().unwrap(),
                                Err(_) => 999,
                                // default user id is 999
                            };
                            c.id = user_id;

                            // set the correct user id if we're letting the client connect

                            println!("client authenticated: {:#?}", c);
                            clients.lock().unwrap().add_client(c);

                            match stream.write(format!("SUCC {}\n", user_id).as_bytes()) {
                                Ok(_) => {}
                                Err(_) => {}
                            }
                        } else {
                            println!("closing connection. client has different version");
                            break 'connection;
                        } // only accept client if they're using the same version as us
                    } // client authentication
                    _ => {
                        println!("closing connection. invalid command");
                        break 'connection;
                    }
                }
            } // don't allow clients to authenticate more than once

            // println!("{}", str::from_utf8(&data).unwrap());
            // println!("clients: {:#?}", clients);
            // debug
            data = [0 as u8; 256]; // clear the buffer on each iteration
            true
        }
        Err(_) => {
            println!(
                "An error occurred, terminating connection with {}",
                client.socket_addr
            );
            break 'connection;
        }
    } {}
}

#[derive(Debug)]
struct Client {
    ip: SocketAddr,
    stream: TcpStream,
    username: String,
    id: usize,
    version: String,
}

struct ClientHandler {
    client_list: Arc<Mutex<Vec<Client>>>,
    socket_addr: SocketAddr,
}

impl Drop for ClientHandler {
    fn drop(&mut self) {
        match self.client_list.lock() {
            Ok(mut client_list_lock) => {
                client_list_lock.remove_client(self.socket_addr);
            }
            Err(_) => (),
        }
    }
} // automatically drop connections when clients go out of scope

impl PartialEq for Client {
    fn eq(&self, other: &Self) -> bool {
        self.ip == other.ip
    }
}

trait ClientMethods {
    fn connected(&self, ip: SocketAddr) -> bool;
    fn get_client_by_username(&self, username: String) -> Option<&Client>;
    fn get_client_by_userid(&self, username: String) -> Option<&Client>;
    fn get_client_by_ip(&self, ip: SocketAddr) -> Option<&Client>;
    fn add_client(&mut self, client: Client);
    fn send_conn(&self, client: &Client);
    fn remove_client(&mut self, socket: SocketAddr);
    fn send_disc(&self, client: &Client);
    fn get_client_index(&self, client: &Client) -> Option<usize>;
}

impl ClientMethods for Vec<Client> {
    fn get_client_by_ip(&self, ip: SocketAddr) -> Option<&Client> {
        for c in self.iter() {
            if c.ip == ip {
                return Some(c);
            } // client found
        }
        None
    }

    fn connected(&self, ip: SocketAddr) -> bool {
        match self.get_client_by_ip(ip) {
            Some(_) => true,
            None => false,
        }
    }
    fn get_client_by_username(&self, username: String) -> Option<&Client> {
        for c in self.iter() {
            if c.username == username {
                return Some(c);
            }
        }
        None
    }
    fn get_client_by_userid(&self, user_id: String) -> Option<&Client> {
        for c in self.iter() {
            if c.id.to_string() == user_id {
                return Some(c);
            }
        }
        None
    }

    fn add_client(&mut self, client: Client) {
        &self.send_conn(&client);
        &self.push(client);
    }

    fn send_conn(&self, client: &Client) {
        let send = format!("CONN {}:{}\n", client.id, client.username);
        for c in self.iter() {
            if c.id != client.id {
                match c.stream.try_clone().unwrap().write(send.as_bytes()) {
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        } // only send to clients other than the connecting client
    }

    fn get_client_index(&self, client: &Client) -> Option<usize> {
        self.iter()
            .position(|client_list| client_list.id == client.id)
    }

    fn remove_client(&mut self, socket: SocketAddr) {
        match self.get_client_by_ip(socket) {
            Some(client) => {
                match self.get_client_index(&client) {
                    Some(i) => {
                        &self.send_disc(&client);
                        &self.remove(i);
                    }
                    None => {}
                } // remove the client and tell other clients they disconnected
            }
            None => {}
        }
    }

    fn send_disc(&self, client: &Client) {
        let send = format!("DISC {}\n", client.id);
        for c in self.iter() {
            match c.stream.try_clone().unwrap().write(send.as_bytes()) {
                Ok(_) => {}
                Err(_) => {}
            }
        }
    }
}

fn main() {
    let listener = TcpListener::bind("127.0.0.1:3333").unwrap();
    let clients = Arc::new(Mutex::new(vec![Client {
        stream: TcpStream::connect("127.0.0.1:3333").unwrap(),
        ip: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0),
        id: 0,
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
