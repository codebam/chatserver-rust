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
    task::block_on(accept_loop("127.0.0.1:8080"))
}

async fn accept_loop(addr: impl ToSocketAddrs) -> Result<()> {
    let listener = TcpListener::bind(addr).await?;
    // wait until we recieve a connection

    let (broker_sender, broker_receiver) = mpsc::unbounded();
    let broker = task::spawn(broker_loop(broker_receiver));
    // create our broker for recieving and sending data

    let mut incoming = listener.incoming();
    while let Some(stream) = incoming.next().await {
        let stream = stream?;
        println!("Accepting from: {}", stream.peer_addr()?);
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

    let verb = packet.verb.unwrap();
    let from = packet.from.unwrap();
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

    while let Some(line) = lines.next().await {
        let line = line?;
        let packet: Packet = serde_json::from_str(&line)?;
        // read the next packet

        let response_packet: Option<Packet> = match packet
            .verb
            .unwrap_or(Err("peer did not specify verb")?)
            .as_ref()
        {
            "SEND" => {
                let dest: Vec<String> = packet.to.unwrap_or(Err("peer didn't give us a dest")?);
                let msg: String = packet
                    .data
                    .unwrap_or(Err("peer didn't send any data")?)
                    .trim()
                    .to_string();

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
            _ => Err("peer did not specify a valid verb")?,
        };
        // handle the packet

        let uw_packet = match response_packet {
            Some(packet) => packet,
            None => Err("no packet to send")?,
        };
        broker
            .send(Event::NewPacket {
                number: uw_packet.number,
                version: uw_packet.version,
                from: uw_packet.from,
                to: uw_packet.to,
                verb: uw_packet.verb,
                data: uw_packet.data,
            })
            .await
            .unwrap();
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
struct Packet {
    number: Option<usize>,
    version: Option<usize>,
    from: Option<String>,
    to: Option<Vec<String>>,
    verb: Option<String>,
    data: Option<String>,
}

enum Event {
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

async fn broker_loop(mut events: Receiver<Event>) {
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

                        let msg = serde_json::to_string(&packet).unwrap();
                        // serialize it

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
