use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use serde::Serialize;

use crate::daemon_cmd::DAEMON_PORT;
use crate::fs_ops::{self, FsService};
use crate::requests;
use crate::responses;

const MAX_FRAME: u64 = 16 * 1024 * 1024;

/// Command line: `androidfs_server [--token T] [--root PATH] [--port N] [PATH]`.
/// A bare `PATH` is accepted for compatibility with older hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub root: Option<PathBuf>,
    pub token: String,
    pub port: u16,
}

pub fn parse_args(args: &[String]) -> Config {
    let mut config = Config {
        root: None,
        token: String::new(),
        port: DAEMON_PORT,
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--token" => {
                if let Some(value) = iter.next() {
                    config.token = value.clone();
                }
            }
            "--root" => {
                if let Some(value) = iter.next() {
                    if !value.trim().is_empty() {
                        config.root = Some(PathBuf::from(value.trim()));
                    }
                }
            }
            "--port" => {
                if let Some(value) = iter.next() {
                    if let Ok(port) = value.parse() {
                        config.port = port;
                    }
                }
            }
            other if !other.starts_with("--") && !other.trim().is_empty() => {
                config.root = Some(PathBuf::from(other.trim()));
            }
            _ => {}
        }
    }
    config
}

fn current_uid() -> u32 {
    unsafe { libc::getuid() as u32 }
}

pub fn build_hello(config: &Config, choice: &fs_ops::RootChoice) -> responses::Hello {
    responses::Hello {
        protocol: responses::PROTOCOL_VERSION,
        token: config.token.clone(),
        uid: current_uid(),
        root: choice.path.to_string_lossy().into_owned(),
        root_entries: fs_ops::count_entries(&choice.path),
        root_fallback: choice.fallback,
        note: choice.note.clone(),
    }
}

pub fn run() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let config = parse_args(&args);
    let choice = FsService::choose_root(config.root.as_deref());
    if !choice.path.is_dir() {
        eprintln!(
            "AndroidFS server: root {} is not a directory",
            choice.path.display()
        );
        std::process::exit(1);
    }
    let hello = build_hello(&config, &choice);
    eprintln!(
        "AndroidFS server: uid {}, root {}, entries {:?}{}",
        hello.uid,
        hello.root,
        hello.root_entries,
        choice
            .note
            .as_ref()
            .map(|note| format!(" ({note})"))
            .unwrap_or_default()
    );

    let listener = match TcpListener::bind(("127.0.0.1", config.port)) {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!(
                "AndroidFS server: failed to bind 127.0.0.1:{}: {err}",
                config.port
            );
            std::process::exit(1);
        }
    };
    eprintln!("AndroidFS server listening on 127.0.0.1:{}", config.port);

    loop {
        let (mut client, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("AndroidFS server: accept failed: {err}");
                std::process::exit(1);
            }
        };
        let mut service = FsService::new(choice.path.clone());
        match serve_session(&mut client, &mut service, &hello) {
            // A connection that closed before sending anything is just a probe
            // (adb forward health check, port scan). Keep waiting for the real one.
            SessionEnd::Probe => continue,
            SessionEnd::Finished => {
                eprintln!("AndroidFS server: client disconnected");
                return;
            }
            SessionEnd::Failed(err) => {
                eprintln!("AndroidFS server: {err}");
                return;
            }
        }
    }
}

enum SessionEnd {
    Probe,
    Finished,
    Failed(std::io::Error),
}

fn serve_session(
    client: &mut TcpStream,
    service: &mut FsService,
    hello: &responses::Hello,
) -> SessionEnd {
    let mut served = 0u64;
    loop {
        match serve_one(client, service, hello) {
            Ok(()) => served += 1,
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                return if served == 0 {
                    SessionEnd::Probe
                } else {
                    SessionEnd::Finished
                };
            }
            Err(err) => {
                return if served == 0 {
                    SessionEnd::Probe
                } else {
                    SessionEnd::Failed(err)
                };
            }
        }
    }
}

fn serve_one(
    client: &mut TcpStream,
    service: &mut FsService,
    hello: &responses::Hello,
) -> std::io::Result<()> {
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
        requests::Request::Hello => write_response(client, Ok(hello.clone()))?,
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

    use super::{build_hello, serve_one, Config};
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
            let mut service = FsService::new(root_for_server.clone());
            let hello = build_hello(
                &Config {
                    root: Some(root_for_server.clone()),
                    token: "tok".into(),
                    port: 0,
                },
                &FsService::choose_root(Some(&root_for_server)),
            );
            while serve_one(&mut client, &mut service, &hello).is_ok() {}
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

#[cfg(test)]
mod session_tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};

    use super::{build_hello, parse_args, serve_session, Config, SessionEnd};
    use crate::fs_ops::FsService;
    use crate::requests::Request;
    use crate::responses;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn parses_command_line() {
        let config = parse_args(&args(&["--token", "abc", "--root", "/storage/My Disk"]));
        assert_eq!(config.token, "abc");
        assert_eq!(config.root.as_deref(), Some(std::path::Path::new("/storage/My Disk")));
        assert_eq!(config.port, 12345);

        let legacy = parse_args(&args(&["/sdcard"]));
        assert_eq!(legacy.root.as_deref(), Some(std::path::Path::new("/sdcard")));

        let empty = parse_args(&args(&["--root", "  ", "--port", "4000"]));
        assert!(empty.root.is_none());
        assert_eq!(empty.port, 4000);
    }

    #[test]
    fn probe_connection_does_not_stop_the_daemon() {
        let root = std::env::temp_dir().join(format!("androidfs-hello-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"x").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        let root_for_server = root.clone();
        std::thread::spawn(move || {
            let config = Config {
                root: Some(root_for_server.clone()),
                token: "secret".into(),
                port,
            };
            let choice = FsService::choose_root(config.root.as_deref());
            let hello = build_hello(&config, &choice);
            for _ in 0..2 {
                let (mut client, _) = listener.accept().unwrap();
                let mut service = FsService::new(choice.path.clone());
                let outcome = match serve_session(&mut client, &mut service, &hello) {
                    SessionEnd::Probe => "probe",
                    SessionEnd::Finished => "finished",
                    SessionEnd::Failed(_) => "failed",
                };
                tx.send(outcome).unwrap();
            }
        });

        // 1) a connection that sends nothing, like adb probing the forward
        drop(TcpStream::connect(("127.0.0.1", port)).unwrap());
        assert_eq!(rx.recv().unwrap(), "probe");

        // 2) the real session starts with Hello
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let encoded = bincode::serialize(&Request::Hello).unwrap();
        stream.write_u64::<BigEndian>(encoded.len() as u64).unwrap();
        stream.write_all(&encoded).unwrap();
        let length = stream.read_u64::<BigEndian>().unwrap() as usize;
        let mut buf = vec![0u8; length];
        stream.read_exact(&mut buf).unwrap();
        let hello: responses::Result<responses::Hello> = bincode::deserialize(&buf).unwrap();
        let hello = hello.unwrap();
        assert_eq!(hello.token, "secret");
        assert_eq!(hello.protocol, responses::PROTOCOL_VERSION);
        assert_eq!(hello.root, root.to_string_lossy());
        assert_eq!(hello.root_entries, Some(2));
        assert!(!hello.root_fallback);
        assert!(hello.note.is_none());

        drop(stream);
        assert_eq!(rx.recv().unwrap(), "finished");
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
