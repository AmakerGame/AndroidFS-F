use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use serde::Serialize;

use crate::fs_ops::FsService;
use crate::requests;
use crate::responses;

const MAX_FRAME: u64 = 16 * 1024 * 1024;

pub fn run() {
    let root = match std::env::args().nth(1) {
        Some(arg) if !arg.trim().is_empty() => std::path::PathBuf::from(arg),
        _ => FsService::detect_root(),
    };
    if !root.is_dir() {
        eprintln!(
            "AndroidFS server: root {} is not a directory",
            root.display()
        );
        std::process::exit(1);
    }
    eprintln!("AndroidFS server listening on 127.0.0.1:12345");
    eprintln!("AndroidFS server root: {}", root.display());

    let listener = match TcpListener::bind("127.0.0.1:12345") {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("AndroidFS server: failed to bind 127.0.0.1:12345: {err}");
            std::process::exit(1);
        }
    };
    let (mut client, _) = match listener.accept() {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("AndroidFS server: accept failed: {err}");
            std::process::exit(1);
        }
    };

    let mut service = FsService::new(root);
    loop {
        if let Err(err) = serve_one(&mut client, &mut service) {
            if err.kind() != std::io::ErrorKind::UnexpectedEof {
                eprintln!("AndroidFS server: {err}");
            } else {
                eprintln!("AndroidFS server: client disconnected");
            }
            return;
        }
    }
}

fn serve_one(client: &mut TcpStream, service: &mut FsService) -> std::io::Result<()> {
    let length = client.read_u64::<BigEndian>()?;
    if length > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "request frame is too large",
        ));
    }
    let mut buf = vec![0u8; length as usize];
    client.read_exact(&mut buf)?;
    let request = bincode::deserialize::<requests::Request>(&buf).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
    })?;

    match request {
        requests::Request::List(path) => write_response(client, service.list_files(&path))?,
        requests::Request::Stat(path) => write_response(client, service.stat_file(&path))?,
        requests::Request::Delete(path) => write_response(client, service.delete(&path))?,
        requests::Request::CreateFile(req) => write_response(client, service.create_file(&req))?,
        requests::Request::CreateDirectory(req) => {
            write_response(client, service.create_directory(&req))?
        }
        requests::Request::Open(req) => write_response(client, service.open_file(&req))?,
        requests::Request::Close(handle) => write_response(client, service.close(handle))?,
        requests::Request::Move(req) => write_response(client, service.move_file(&req))?,
        requests::Request::GetFreeSpace => write_response(client, service.free_space())?,
        requests::Request::SetEndOfFile(req) => {
            write_response(client, service.set_end_of_file(req.handle, req.len))?
        }
        requests::Request::Read(req) => match service.read_at(req.handle, req.offset, req.len) {
            Ok(bytes) => {
                write_response::<responses::ReadFile>(client, Ok(bytes.len() as u32))?;
                client.write_all(&bytes)?;
            }
            Err(err) => write_response::<responses::ReadFile>(client, Err(err))?,
        },
        requests::Request::Write(req) => {
            // The payload follows the request frame. It is always consumed
            // before answering, so a failing write cannot desync the stream.
            if req.len > MAX_FRAME {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "write payload is too large",
                ));
            }
            let mut data = vec![0u8; req.len as usize];
            client.read_exact(&mut data)?;
            let result = service.write_at(req.handle, req.offset, &data).map(|_| ());
            if let Err(err) = &result {
                eprintln!("AndroidFS server: write failed: {err:?}");
            }
            write_response(client, result)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod framing_tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

    use super::serve_one;
    use crate::fs_ops::FsService;
    use crate::requests::{self, Request};
    use crate::responses;

    fn exchange(stream: &mut TcpStream, request: &Request) -> Vec<u8> {
        let encoded = bincode::serialize(request).unwrap();
        stream.write_u64::<BigEndian>(encoded.len() as u64).unwrap();
        stream.write_all(&encoded).unwrap();
        let length = stream.read_u64::<BigEndian>().unwrap() as usize;
        let mut buf = vec![0u8; length];
        stream.read_exact(&mut buf).unwrap();
        buf
    }

    #[test]
    fn tcp_create_write_read_roundtrip() {
        let root = std::env::temp_dir().join(format!("androidfs-tcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let root_for_server = root.clone();
        std::thread::spawn(move || {
            let (mut client, _) = listener.accept().unwrap();
            let mut service = FsService::new(root_for_server);
            while serve_one(&mut client, &mut service).is_ok() {}
        });

        std::thread::sleep(Duration::from_millis(50));
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();

        let raw = exchange(
            &mut stream,
            &Request::CreateDirectory(requests::CreateDirectory {
                path: "/docs".into(),
                exclusive: true,
            }),
        );
        let created: responses::Result<()> = bincode::deserialize(&raw).unwrap();
        assert_eq!(created, Ok(()));

        let raw = exchange(
            &mut stream,
            &Request::CreateFile(requests::CreateFile {
                path: "/docs/hello.txt".into(),
                truncate: false,
                exclusive: true,
            }),
        );
        let handle: responses::Result<u32> = bincode::deserialize(&raw).unwrap();
        let handle = handle.unwrap();

        let payload = b"androidfs";
        let encoded = bincode::serialize(&Request::Write(requests::WriteFile {
            handle,
            offset: 0,
            len: payload.len() as u64,
        }))
        .unwrap();
        stream.write_u64::<BigEndian>(encoded.len() as u64).unwrap();
        stream.write_all(&encoded).unwrap();
        stream.write_all(payload).unwrap();
        let length = stream.read_u64::<BigEndian>().unwrap() as usize;
        let mut buf = vec![0u8; length];
        stream.read_exact(&mut buf).unwrap();
        let ack: responses::Result<()> = bincode::deserialize(&buf).unwrap();
        assert_eq!(ack, Ok(()));

        let encoded = bincode::serialize(&Request::Read(requests::ReadFile {
            handle,
            offset: 0,
            len: 64,
        }))
        .unwrap();
        stream.write_u64::<BigEndian>(encoded.len() as u64).unwrap();
        stream.write_all(&encoded).unwrap();
        let length = stream.read_u64::<BigEndian>().unwrap() as usize;
        let mut buf = vec![0u8; length];
        stream.read_exact(&mut buf).unwrap();
        let n: responses::Result<u32> = bincode::deserialize(&buf).unwrap();
        let n = n.unwrap() as usize;
        let mut data = vec![0u8; n];
        stream.read_exact(&mut data).unwrap();
        assert_eq!(data, payload);

        let _ = std::fs::remove_dir_all(&root);
    }
}

fn write_response<T: Serialize>(
    client: &mut TcpStream,
    response: responses::Result<T>,
) -> std::io::Result<()> {
    let encoded = bincode::serialize(&response).map_err(|err| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string())
    })?;
    client.write_u64::<BigEndian>(encoded.len() as u64)?;
    client.write_all(&encoded)?;
    Ok(())
}
