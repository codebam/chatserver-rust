mod lib;
pub use crate::lib::hashing;

use std::{
    collections::hash_map::{Entry, HashMap},
    sync::Arc,
    str,
};

use async_std::{
    io::BufReader,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    prelude::*,
    task,
};
use futures::{channel::mpsc, select, FutureExt, SinkExt};

use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sender<T> = mpsc::UnboundedSender<T>;
type Receiver<T> = mpsc::UnboundedReceiver<T>;

const SERVER_VERSION: usize = 2;

#[derive(Serialize, Deserialize, Debug)]
struct ConnList {
    conn: Vec<String>
}

struct Peer {
    sender: Sender<String>,
    next_send_packet_id: usize,
    packet_list: Vec<Packet>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Packet {
    number: Option<usize>,
    version: Option<usize>,
    from: Option<String>,
    to: Option<Vec<String>>,
    verb: Option<String>,
    data: Option<String>,
    checksum: Option<String>,
}

enum Event {
    ClientDisconnect {
        username: Option<String>,
    },
    NewPacketWho {
        version: Option<usize>,
        from: Option<String>,
        verb: Option<String>,
    },
    NewPacketAll {
        version: Option<usize>,
        from: Option<String>,
        verb: Option<String>,
        data: Option<String>,
    },
    ResendPacket {
        to: Option<Vec<String>>,
        number: Option<usize>,
    },
    NewPacket {
        version: Option<usize>,
        from: Option<String>,
        to: Option<Vec<String>>,
        verb: Option<String>,
        data: Option<String>,
    },
    // by making these all Option<>, when packets come in without data fields
    // they get populated as None so we can handle them instead of crashing :)
    NewPeer {
        name: String,
        stream: Arc<TcpStream>,
        shutdown: Receiver<()>,
    },
}

pub(crate) fn main() -> Result<()> {
    task::block_on(accept_loop("127.0.0.1:59944"))
}

async fn accept_loop(addr: impl ToSocketAddrs) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    // wait until we recieve a connection

    let (broker_sender, broker_receiver) = mpsc::unbounded();
    let broker = task::spawn(broker_loop(broker_sender.clone(), broker_receiver));
    // create our broker for recieving and sending data

    let mut incoming = listener.incoming();
    while let Some(stream) = incoming.next().await {
        let stream = stream?;
        spawn_and_log_error(connection_loop(broker_sender.clone(), stream));
    }
    // once our peer sends data, send it through our connection loop

    drop(broker_sender);
    // drop the broker_sender when they stop sending us data

    broker.await;
    Ok(())
    // gracefully close the connection
}

async fn connection_loop(mut broker: Sender<Event>, stream: TcpStream) -> Result<()> {
    let stream = Arc::new(stream);
    let reader = BufReader::new(&*stream);
    let mut lines = reader.lines();
    // create a new reader for our TCP stream

    let mut next_recv_packet_id: usize = 0;
    // keep track of packet recieved numbers

    let packet: Packet = match lines.next().await {
        None => Err("peer disconnected without sending packet")?,
        Some(line) => serde_json::from_str(&line?)?,
    };
    // read next packet and deserialize it

    let version: usize = match packet.version {
        Some(version) => {
            if version == SERVER_VERSION {
                SERVER_VERSION
            } else {
                Err("invalid version number")?
            }
        }
        None => Err("peer didn't specify version")?,
    };
    drop(version);
    let verb: String = match packet.verb {
        Some(verb) => verb,
        None => Err("peer didn't specify verb")?,
    };
    let from: String = match packet.from {
        Some(from) => from,
        None => Err("peer didn't specify sender")?,
    };
    let name: String = match verb.clone().as_ref() {
        "LOGN" => from.trim(),
        _ => Err("peer did not login")?,
    }
    .to_string();
    drop(verb);
    drop(from);
    // get username from packet

    let (_shutdown_sender, shutdown_receiver) = mpsc::unbounded::<()>();
    broker
        .send(Event::NewPeer {
            name: name.clone(),
            stream: Arc::clone(&stream),
            shutdown: shutdown_receiver,
        })
        .await
        .unwrap();
    // send new peer to our broker to keep track of peer list

    broker
        .send(Event::NewPacket {
            version: Some(SERVER_VERSION),
            from: Some(String::from("root")),
            to: Some(vec![name.clone().to_string()]),
            verb: Some(String::from("SUCC")),
            data: None,
        })
        .await
        .unwrap();
    // send peer success packet after login

    while let Some(line) = lines.next().await {
        let line = line?;

        let packet: Packet = serde_json::from_str(&line)?;
        // read the next packet

        let number = match packet.number {
            Some(packet_number) => {
                packet_number
            },
            None => Err(format!("peer did not supply packet number:\n{:#?}", packet))?,
        };

        if next_recv_packet_id != number {
            let resend_last_packets: usize = number - next_recv_packet_id;
            for i in (next_recv_packet_id - resend_last_packets)..next_recv_packet_id {
                let data = format!("{}", i);
                let to = vec![packet.from.clone().unwrap()];
                broker.send(Event::NewPacket {
                    version: Some(SERVER_VERSION),
                    from: Some(String::from("root")),
                    to: Some(to),
                    verb: Some(String::from("RESEND")),
                    data: Some(data),

                }).await?;
            }
        }
        // ask to resend missed packets if the numbers aren't sequential

        let checksum = match packet.checksum {
            Some(sum) => sum,
            None => Err(format!("peer did not supply checksum:\n{:#?}", packet))?
        };

        let data = match packet.data.clone() {
            Some(d) => d,
            None => String::from("")
        };

        let calculated_checksum = hashing::get_hash(data);

        // println!("checksum            {}", checksum);
        // println!("calculated_checksum {}", calculated_checksum);
        match checksum == calculated_checksum {
            true => (),
            false => {
                println!("requesting resend of packet number {}", number);
                let to = vec![packet.from.clone().unwrap()];
                broker.send(Event::NewPacket {
                    version: Some(SERVER_VERSION),
                    from: Some(String::from("root")),
                    to: Some(to),
                    verb: Some(String::from("RESEND")),
                    data: Some(number.to_string()),

                }).await?;
            }
            // request resend of miscalculated checksum packet
        }

        let verb = match packet.verb {
            Some(verb) => verb,
            None => Err("peer did not specify verb")?
        };
        match verb.as_ref() {
            "SEND" => {
                let dest: Vec<String> = match packet.to {
                    Some(to) => to,
                    None => Err("peer didn't give us a dest")?,
                };
                let msg: String = match packet.data {
                    Some(data) => data.trim().to_string(),
                    None => Err("peer didn't send any data")?
                };
                broker.send(Event::NewPacket {
                    version: Some(SERVER_VERSION),
                    from: Some(name.clone()),
                    to: Some(dest),
                    verb: Some(String::from("RECV")),
                    data: Some(msg.clone()),
                }).await.unwrap();
            }
            "SALL" => {
                let msg: String = match packet.data {
                    Some(data) => data.trim().to_string(),
                    None => Err("peer didn't send any data")?
                };
                broker.send(Event::NewPacketAll {
                    version: Some(SERVER_VERSION),
                    from: Some(name.clone()),
                    verb: Some(String::from("RECV")),
                    data: Some(msg),
                }).await.unwrap();
            },
            "WHOO" => {
                broker.send(Event::NewPacketWho {
                    version: Some(SERVER_VERSION),
                    from: Some(name.clone()),
                    verb: Some(String::from("CONN")),
                }).await.unwrap();
            },
            "DISC" => {
                broker.send(Event::ClientDisconnect {
                    username: Some(name.clone()),
                }).await.unwrap();
                return Ok(());
            },
            "RESEND" => {
                broker.send(Event::ResendPacket {
                    to: Some(vec![name.clone()]),
                    number: Some(packet.data.unwrap().parse::<usize>().unwrap())
                }).await.unwrap();
            },
            verb => Err(format!("peer did not specify a valid verb: {}", verb))?,
        };
        // handle the packet

        next_recv_packet_id += 1;
        // increment the next packet id
    }
    match broker.send(Event::NewPacket {
        version: Some(SERVER_VERSION),
        from: None,
        to: None,
        data: Some(name.clone()),
        verb: Some(String::from("DISC")),
    }).await {
        Ok(_) => (),
        Err(_) => (),
    }
    Ok(())
}

async fn connection_writer_loop(
    messages: &mut Receiver<String>,
    stream: Arc<TcpStream>,
    mut shutdown: Receiver<()>,
) -> Result<()> {
    let mut stream = &*stream;
    loop {
        select! {
            msg = messages.next().fuse() => match msg {
                Some(msg) => stream.write_all(msg.as_bytes()).await?,
                None => break,
            },
            void = shutdown.next().fuse() => match void {
                Some(void) => match void {
                    void => (),
                    _ => ()
                },
                None => break,
            }
        }
    }
    Ok(())
}

async fn broker_loop(mut broker: Sender<Event>, mut events: Receiver<Event>) {
    let (disconnect_sender, mut disconnect_receiver) =
        mpsc::unbounded::<(String, Receiver<String>)>();
    let mut peers: HashMap<String, Peer> = HashMap::new();

    loop {
        let event = select! {
            event = events.next().fuse() => match event {
                None => break,
                Some(event) => event,
            },
            // get the event
            disconnect = disconnect_receiver.next().fuse() => {
                let (name, _pending_messages) = disconnect.unwrap();
                assert!(peers.remove(&name).is_some());
                continue;
            },
            // remove disconnected peers
        };
        match event {
            Event::ClientDisconnect {
                username,
            } => {
                let to = peers.keys().cloned().collect::<Vec<_>>();
                broker.send(Event::NewPacket {
                    version: Some(SERVER_VERSION),
                    from: None,
                    to: Some(to),
                    data: username,
                    verb: Some(String::from("DISC")),
                }).await.unwrap();
            }
            Event::NewPacketWho {
                version,
                from,
                verb,
            } => {
                let connected_peers = ConnList { conn: peers.keys().cloned().collect::<Vec<_>>() };
                let data = serde_json::to_string(&connected_peers).unwrap();
                broker.send(Event::NewPacket {
                    version: version,
                    from: None,
                    to: Some(vec![from.unwrap()]),
                    verb: verb,
                    data: Some(data),
                }).await.unwrap();
                // send it back to the broker
            }
            Event::NewPacketAll {
                version,
                from,
                verb,
                data,
            } => {
                let to: Vec<String> = peers.keys().cloned().collect::<Vec<_>>();
                // get a list of all peers

                broker.send(Event::NewPacket {
                    version: version,
                    from: from,
                    to: Some(to),
                    verb: verb,
                    data: data,
                }).await.unwrap();
                // send it back to the broker (look one line down)
            }
            Event::ResendPacket {
                to,
                number,
            } => {
                let _to = to.clone();
                match to {
                    Some(addrs) => {
                        for addr in addrs {
                            if let Some(peer) = peers.get_mut(&addr) {
                                let n = number.unwrap();
                                let old_packet = &peer.packet_list[n];
                                broker.send(Event::NewPacket {
                                    version: old_packet.version,
                                    from: old_packet.from.clone(),
                                    to: old_packet.to.clone(),
                                    verb: old_packet.verb.clone(),
                                    data: old_packet.data.clone(),
                                }).await.unwrap();
                                // send the old packet back through our event reciever
                            }
                        }
                    },
                    None => (),
                }

            }
            Event::NewPacket {
                version,
                from,
                to,
                verb,
                data,
            } => {
                let _to = to.clone();
                match to {
                    Some(addrs) => {
                        for addr in addrs {
                            if let Some(peer) = peers.get_mut(&addr) {
                                let checksum = match data.clone() {
                                    Some(d) => Some(hashing::get_hash(d)),
                                    None => Some(hashing::get_hash(String::from("")))

                                };
                                let packet = Packet {
                                    number: Some(peer.next_send_packet_id),
                                    version: version,
                                    from: from.clone(),
                                    to: _to.clone(),
                                    verb: verb.clone(),
                                    data: data.clone(),
                                    checksum: checksum,
                                };
                                // create our packet

                                let msg = format!("{}\n", serde_json::to_string(&packet).unwrap());
                                // serialize it and append a newline

                                match peer.sender.send(msg).await {
                                    Ok(_) => (),
                                    Err(_) => (),
                                }
                                // then send it off

                                peer.next_send_packet_id = peer.next_send_packet_id + 1;
                                // increment the next packet id

                                peer.packet_list.push(packet);
                                // move the packet to our packet_list
                            }
                        }
                    },
                    None => ()
                }
            }
            Event::NewPeer {
                name,
                stream,
                shutdown,
            } => match peers.entry(name.clone()) {
                Entry::Occupied(..) => (),
                Entry::Vacant(entry) => {
                    let (client_sender, mut client_receiver) = mpsc::unbounded();
                    let peer = Peer{sender: client_sender, next_send_packet_id: 1, packet_list: Vec::new()};
                    entry.insert(peer);
                    let mut disconnect_sender = disconnect_sender.clone();
                    spawn_and_log_error(async move {
                        let res =
                            connection_writer_loop(&mut client_receiver, stream, shutdown).await;
                        disconnect_sender
                            .send((name, client_receiver))
                            .await
                            .unwrap();
                        res
                    });
                }
            },
            // add new peers to our hashmap of connected peers
        }
    }
    drop(peers);
    drop(disconnect_sender);
    while let Some((_name, _pending_messages)) = disconnect_receiver.next().await {}
}

fn spawn_and_log_error<F>(fut: F) -> task::JoinHandle<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    task::spawn(async move {
        if let Err(e) = fut.await {
            eprintln!("{}", e)
        }
    })
}
