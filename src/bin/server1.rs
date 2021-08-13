use std::net::{TcpListener, TcpStream, Shutdown};
use std::io::Read;
use std::fs;

fn handle_client(mut stream: TcpStream) {
    let mut data= Vec::new();
    match stream.read_to_end(&mut data) {
        Ok(size) => {
            fs::write("received/tcp.csv", &data);
        }
        Err(_) => {
            println!("terminating connection with {}",stream.peer_addr().unwrap());
            stream.shutdown(Shutdown::Both).unwrap();
        }
    }
}

fn main() {
    let listener = TcpListener::bind("0.0.0.0:3333").unwrap();

    for stream in listener.incoming() {
        match stream  {
            Ok(stream) => {
                println!("New connection: {}", stream.peer_addr().unwrap());
                handle_client(stream);
            }
            Err(_) => {
                println!("error connecting to socket ");
            }
        }
    }
    drop(listener);
}