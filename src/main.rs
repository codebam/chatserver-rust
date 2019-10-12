use std::{
    collections::hash_map::{Entry, HashMap},
    sync::Arc,
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

#[derive(Debug)]
enum Void {}

const SERVER_VERSION: usize = 1;
// FooChat Version 1

pub(crate) fn main() -> Result<()> {
    task::block_on(accept_loop("127.0.0.1:682651"))
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

    let (_shutdown_sender, shutdown_receiver) = mpsc::unbounded::<Void>();
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
            number: None, // TODO
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

        let verb = match packet.verb {
            Some(verb) => verb,
            None => Err("peer did not specify verb")?
        };
        let response_packet: Option<Packet> = match verb
            .as_ref()
        {
            "SEND" => {
                let dest: Vec<String> = match packet.to {
                    Some(to) => to,
                    None => Err("peer didn't give us a dest")?,
                };
                let msg: String = match packet.data {
                    Some(data) => data.trim().to_string(),
                    None => Err("peer didn't send any data")?
                };
                let packet: Packet = Packet {
                    number: None, // TODO: unimplemented
                    version: Some(SERVER_VERSION),
                    from: Some(name.clone()),
                    to: Some(dest),
                    verb: Some(String::from("RECV")),
                    data: Some(msg),
                };
                Some(packet)
            }
            "SALL" => {
                let msg: String = match packet.data {
                    Some(data) => data.trim().to_string(),
                    None => Err("peer didn't send any data")?
                };
                broker.send(Event::NewPacketAll {
                    number: None,
                    version: Some(SERVER_VERSION),
                    from: Some(name.clone()),
                    verb: Some(String::from("RECV")),
                    data: Some(msg),
                }).await.unwrap();
                None
            },
            "WHOO" => {
                broker.send(Event::NewPacketWho {
                    number: None,
                    version: Some(SERVER_VERSION),
                    from: Some(name.clone()),
                    verb: Some(String::from("CONN")),
                }).await.unwrap();
                None
            },
            "DISC" => {
                return Ok(());
            },
            _ => Err("peer did not specify a valid verb")?,
        };
        // handle the packet

        match response_packet {
            Some(packet) => {
                broker.send(Event::NewPacket {
                    number: packet.number,
                    version: packet.version,
                    from: packet.from,
                    to: packet.to,
                    verb: packet.verb,
                    data: packet.data,
                }).await.unwrap();
            },
            None => (),
        }; // send a packet if we created one
    }
    Ok(())
}

async fn connection_writer_loop(
    messages: &mut Receiver<String>,
    stream: Arc<TcpStream>,
    mut shutdown: Receiver<Void>,
) -> Result<()> {
    let mut stream = &*stream;
    loop {
        select! {
            msg = messages.next().fuse() => match msg {
                Some(msg) => stream.write_all(msg.as_bytes()).await?,
                None => break,
            },
            void = shutdown.next().fuse() => match void {
                Some(void) => match void {},
                None => break,
            }
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize, Debug)]
struct ConnList {
    conn: Vec<String>
}


#[derive(Serialize, Deserialize, Debug)]
struct Packet {
    number: Option<usize>,
    version: Option<usize>,
    from: Option<String>,
    to: Option<Vec<String>>,
    verb: Option<String>,
    data: Option<String>,
}

enum Event {
    NewPacketWho {
        number: Option<usize>,
        version: Option<usize>,
        from: Option<String>,
        verb: Option<String>,
    },
    NewPacketAll {
        number: Option<usize>,
        version: Option<usize>,
        from: Option<String>,
        verb: Option<String>,
        data: Option<String>,
    },
    NewPacket {
        number: Option<usize>,
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
        shutdown: Receiver<Void>,
    },
}

async fn broker_loop(mut broker: Sender<Event>, mut events: Receiver<Event>) {
    let (disconnect_sender, mut disconnect_receiver) =
        mpsc::unbounded::<(String, Receiver<String>)>();
    let mut peers: HashMap<String, Sender<String>> = HashMap::new();

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
            Event::NewPacketWho {
                number,
                version,
                from,
                verb,
            } => {
                let connected_peers = ConnList { conn: peers.keys().cloned().collect::<Vec<_>>() };
                let data = serde_json::to_string(&connected_peers).unwrap();
                broker.send(Event::NewPacket {
                    number: number,
                    version: version,
                    from: None,
                    to: Some(vec![from.unwrap()]),
                    verb: verb,
                    data: Some(data),
                }).await.unwrap();
                // send it back to the broker
            }
            Event::NewPacketAll {
                number,
                version,
                from,
                verb,
                data,
            } => {
                let to: Vec<String> = peers.keys().cloned().collect::<Vec<_>>();
                // get a list of all peers

                broker.send(Event::NewPacket {
                    number: number,
                    version: version,
                    from: from,
                    to: Some(to),
                    verb: verb,
                    data: data,
                }).await.unwrap();
                // send it back to the broker (look one line down)
            }
            Event::NewPacket {
                number,
                version,
                from,
                to,
                verb,
                data,
            } => {
                let _to = to.clone();
                for addr in to.unwrap() {
                    if let Some(peer) = peers.get_mut(&addr) {
                        let packet = Packet {
                            number: number,
                            version: version,
                            from: from.clone(),
                            to: _to.clone(),
                            verb: verb.clone(),
                            data: data.clone(),
                        };
                        // create our packet

                        let msg = format!("{}\n", serde_json::to_string(&packet).unwrap());
                        // serialize it and append a newline

                        peer.send(msg).await.unwrap();
                        // then send it off
                    }
                    // only run this block if the peer is still connected
                }
                // send to each peer in the vector
            }
            Event::NewPeer {
                name,
                stream,
                shutdown,
            } => match peers.entry(name.clone()) {
                Entry::Occupied(..) => (),
                Entry::Vacant(entry) => {
                    let (client_sender, mut client_receiver) = mpsc::unbounded();
                    entry.insert(client_sender);
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
