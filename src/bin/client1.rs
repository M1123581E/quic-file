use std::net::TcpStream;
use std::io::{Read,Write};
use std::fs::File;
use std::process;

fn main() {
    match TcpStream::connect("35.220.196.147:3333") {
        Ok(mut stream) => {
            println!("successfully connected");
            let mut file = File::open("file/2.csv").unwrap_or_else(|err| {
                println!("problem reading file: {}", err);
                process::exit(1)
            });
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            stream.write_all(&buf).unwrap();
        }
        Err(e) => {
            println!("Failed to connect: {}", e);
        }
    }

    println!("finished");
}