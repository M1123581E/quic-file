// Copyright (C) 2018-2019, Cloudflare, Inc.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//     * Redistributions of source code must retain the above copyright notice,
//       this list of conditions and the following disclaimer.
//
//     * Redistributions in binary form must reproduce the above copyright
//       notice, this list of conditions and the following disclaimer in the
//       documentation and/or other materials provided with the distribution.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS
// IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO,
// THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR
// PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR
// CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
// EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO,
// PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR
// PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF
// LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
// NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
// SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

#[macro_use]
extern crate log;

use std::net::ToSocketAddrs;

use ring::rand::*;
use std::collections::HashMap;
use quiche::Connection;
use std::path::PathBuf;
use std::pin::Pin;
use url::Url;


const MAX_DATAGRAM_SIZE: usize = 1350;
const CHUNK_SIZE: usize = 100;



struct PartialResponse {
    body: Vec<u8>,
    written: usize
}

fn main() {
    let mut buf = [0; 65535];
    let mut out = [0; MAX_DATAGRAM_SIZE];

    let mut args = std::env::args();

    let cmd = &args.next().unwrap();



    if args.len() != 1 {
        println!("Usage: {} URL", cmd);
        println!("\nSee tools/apps/ for more complete implementations.");
        return;
    }

    let url = url::Url::parse(&args.next().unwrap()).unwrap();

    // Setup the event loop.
    let poll = mio::Poll::new().unwrap();
    let mut events = mio::Events::with_capacity(1024);

    // Resolve server address.
    let peer_addr = url.to_socket_addrs().unwrap().next().unwrap();

    // Bind to INADDR_ANY or IN6ADDR_ANY depending on the IP family of the
    // server address. This is needed on macOS and BSD variants that don't
    // support binding to IN6ADDR_ANY for both v4 and v6.
    let bind_addr = match peer_addr {
        std::net::SocketAddr::V4(_) => "0.0.0.0:0",
        std::net::SocketAddr::V6(_) => "[::]:0",
    };

    // Create the UDP socket backing the QUIC connection, and register it with
    // the event loop.
    let socket = std::net::UdpSocket::bind(bind_addr).unwrap();

    let socket = mio::net::UdpSocket::from_socket(socket).unwrap();
    poll.register(
        &socket,
        mio::Token(0),
        mio::Ready::readable(),
        mio::PollOpt::edge(),
    )
        .unwrap();

    // Create the configuration for the QUIC connection.
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();

    // *CAUTION*: this should not be set to `false` in production!!!
    config.verify_peer(false);

    config
        .set_application_protos(
            b"\x0ahq-interop\x05hq-29\x05hq-28\x05hq-27\x08http/0.9",
        )
        .unwrap();

    config.set_max_idle_timeout(50000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(10_000_000);
    config.set_initial_max_stream_data_bidi_local(1_000_000);
    config.set_initial_max_stream_data_bidi_remote(1_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);
    config.set_disable_active_migration(true);

    // Generate a random source connection ID for the connection.
    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    SystemRandom::new().fill(&mut scid[..]).unwrap();

    let scid = quiche::ConnectionId::from_ref(&scid);

    let mut partial_responses: HashMap<u64, PartialResponse> = HashMap::new();



    // Create a QUIC connection and initiate handshake.
    let mut conn =
        quiche::connect(url.domain(), &scid, peer_addr, &mut config).unwrap();

    info!(
        "connecting to {:} from {:} with scid {}",
        peer_addr,
        socket.local_addr().unwrap(),
        hex_dump(&scid)
    );

    let (write, send_info) = conn.send(&mut out).expect("initial send failed");

    while let Err(e) = socket.send_to(&out[..write], &send_info.to) {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            println!("send() would block");
            continue;
        }

        println!("send() failed: {:?}", e);
    }

    println!("written {}", write);

    let req_start = std::time::Instant::now();

    let mut req_sent = 0;

    let mut n = 10;

    loop {
        poll.poll(&mut events, conn.timeout()).unwrap();
        // Read incoming UDP packets from the socket and feed them to quiche,
        // until there are no more packets to read.
        'read: loop {
            // If the event loop reported no events, it means that the timeout
            // has expired, so handle it without attempting to read packets. We
            // will then proceed with the send loop.
            if events.is_empty() {
                info!("timed out");

                conn.on_timeout();
                break 'read;
            }

            let (len, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,

                Err(e) => {
                    // There are no more UDP packets to read, so end the read
                    // loop.
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        println!("recv() would block");
                        break 'read;
                    }

                    println!("recv() failed: {:?}", e);
                },
            };

            println!("got {} bytes", len);

            let recv_info = quiche::RecvInfo { from };

            // Process potentially coalesced packets.
            let read = match conn.recv(&mut buf[..len], recv_info) {
                Ok(v) => v,

                Err(e) => {
                    println!("recv failed: {:?}", e);
                    continue 'read;
                },
            };

            println!("processed {} bytes", read);
        }

        println!("done reading");

        if conn.is_closed() {
            println!("connection closed, {:?}", conn.stats());
            break;
        }

        // Send an HTTP request as soon as the connection is established.
        if conn.is_established() && req_sent <= 0 {
            println!("sending file {}", url.path());

            sendFileMeta(&url, &mut conn);
            //sendFile0(&url, &mut partial_responses, &mut conn);
        }

        if conn.is_in_early_data() || conn.is_established() {
            for stream_id in conn.writable() {
                handle_writable(&mut conn, stream_id, &mut partial_responses);
            }
        }

        // Process all readable streams.
        for s in conn.readable() {
            while let Ok((read, fin)) = conn.stream_recv(s, &mut buf) {
                println!("received {} bytes", read);

                let stream_buf = &buf[..read];



                println!(
                    "stream {} has {} bytes (fin? {})",
                    s,
                    stream_buf.len(),
                    fin
                );



                println!("receive:{}", stream_buf.len());
                println!("{}", unsafe {
                    std::str::from_utf8_unchecked(&stream_buf)
                });

                // The server reported that it has no more data to send, which
                // we got the full response. Close the connection.
                if s == 4  {
                    let content = unsafe {
                        std::str::from_utf8_unchecked(&stream_buf)
                    };

                    if(content.contains("meta_received")) {
                        println!("start to send file");
                        sendFile0(&url, &mut partial_responses, &mut conn);
                    }

                    if(content.contains("file_received")) {
                        println!(
                            "response received in {:?}, closing...",
                            req_start.elapsed()
                        );
                        conn.close(true, 0x00, b"kthxbye").unwrap();
                    }
                }
            }
        }

        // Generate outgoing QUIC packets and send them on the UDP socket, until
        // quiche reports that there are no more packets to be sent.
        loop {
            let (write, send_info) = match conn.send(&mut out) {
                Ok(v) => v,

                Err(quiche::Error::Done) => {
                    println!("done writing");
                    break;
                },

                Err(e) => {
                    println!("send failed: {:?}", e);

                    conn.close(false, 0x1, b"fail").ok();
                    break;
                },
            };

            if let Err(e) = socket.send_to(&out[..write], &send_info.to) {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    println!("send() would block");
                    break;
                }

                println!("send() failed: {:?}", e);
            }

            println!("written {}", write);
        }

        if conn.is_closed() {
            println!("connection closed, {:?}", conn.stats());
            break;
        }
    }
}

fn sendFileMeta(url: &Url, mut conn: &mut Pin<Box<Connection>>) {
    let uri = std::path::Path::new(url.path());
    let mut path = std::path::PathBuf::from("file");

    for c in uri.components() {
        if let std::path::Component::Normal(v) = c {
            path.push(v)
        }
    }

    let file_len = std::fs::metadata(path.as_path()).unwrap().len();
    let file_name = path.file_name().unwrap().to_str().unwrap().to_owned();

    let body = format!("{}:{}:{}", file_name, file_len, CHUNK_SIZE);

    let written = match conn.stream_send(4, body.as_bytes(), true) {
        Ok(v) => v,

        Err(quiche::Error::Done) => 0,

        Err(e) => {
            println!("{} stream send failed {:?}", conn.trace_id(), e);
            return;
        },
    };





}


fn sendFile0(url: &Url, mut partial_responses: &mut HashMap<u64, PartialResponse>, mut conn: &mut Pin<Box<Connection>>) {
    let uri = std::path::Path::new(url.path());
    let mut path = std::path::PathBuf::from("file");

    for c in uri.components() {
        if let std::path::Component::Normal(v) = c {
            path.push(v)
        }
    }

    let body = std::fs::read(path.as_path())
        .unwrap_or_else(|_| b"Not Found!\r\n".to_vec());

    let mut stream_index = 6;
    for chuck in body.chunks(body.len() / CHUNK_SIZE) {
        println!("{} stream has {} size", stream_index, chuck.len());
        sendFile(&mut partial_responses, &mut conn, chuck.to_vec(), stream_index);
        stream_index = stream_index + 2;
    }


    // conn.stream_send(HTTP_REQ_STREAM_ID, req.as_bytes(), false)
    //     .unwrap();
}

fn sendFile(partial_responses: &mut HashMap<u64, PartialResponse>, conn: &mut Pin<Box<Connection>>, body: Vec<u8>, stream_id: u64) {
    // let body = std::fs::read(path.as_path())
    //     .unwrap_or_else(|_| b"Not Found!\r\n".to_vec());


    // let steam_n = 5;
    // let n = body.len();
    // let size = n / steam_n;
    // let i = 0;
    //
    // let parts = body.chunks(size).map(|x| x.to_vec()).collect();
    let written = match conn.stream_send(stream_id, &body, true) {
        Ok(v) => v,

        Err(quiche::Error::Done) => 0,

        Err(e) => {
            error!("{} stream send failed {:?}", conn.trace_id(), e);
            return;
        },
    };
    if written < body.len() {
        println!("stream；{} -- body:{} -- written:{}", stream_id,  body.len(), written);
        let response = PartialResponse { body, written };
        partial_responses.insert(stream_id, response);
    }
}

fn hex_dump(buf: &[u8]) -> String {
    let vec: Vec<String> = buf.iter().map(|b| format!("{:02x}", b)).collect();

    vec.join("")
}

/// Handles incoming HTTP/0.9 requests.

/// Handles newly writable streams.
fn handle_writable(conn: &mut std::pin::Pin<Box<quiche::Connection>>, stream_id: u64,  partial_responses: &mut HashMap<u64, PartialResponse>) {


    debug!("{} stream {} is writable", conn.trace_id(), stream_id);

    if !partial_responses.contains_key(&stream_id) {
        return;
    }

    let resp = partial_responses.get_mut(&stream_id).unwrap();
    let body = &resp.body[resp.written..];
    // println!("writable body:{}", body.len());

    let written = match conn.stream_send(stream_id, &body, true) {
        Ok(v) => v,

        Err(quiche::Error::Done) => 0,

        Err(e) => {
            partial_responses.remove(&stream_id);

            error!("{} stream send failed {:?}", conn.trace_id(), e);
            return;
        },
    };

    resp.written += written;

    if resp.written == resp.body.len() {
        partial_responses.remove(&stream_id);
    }
}