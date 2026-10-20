//! One client connection: a thread that reads requests and a thread that writes what the server
//! queues for it, so a client that stops reading never stalls the server.

use std::io::Write;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use orma_proto::frame::{self, Kind};
use orma_proto::msg::*;

use super::Core;
use crate::record::export;
use crate::record::replay::Replay;

/// Frames queued for one client; a client this far behind is dropped.
const QUEUE: usize = 256;

pub(super) fn start(core: &Arc<Mutex<Core>>, stream: UnixStream) {
    stream
        .set_nonblocking(false)
        .expect("fcntl on an accepted socket");
    let writer = stream.try_clone().expect("dup of an accepted socket");
    let control = stream.try_clone().expect("dup of an accepted socket");
    let (tx, rx) = mpsc::sync_channel(QUEUE);
    let id = core.lock().unwrap().client_add(tx, control);
    thread::spawn(move || write(writer, rx));
    let core = Arc::clone(core);
    thread::spawn(move || {
        serve(&core, id, stream);
        core.lock().unwrap().client_remove(id);
    });
}

fn write(mut stream: UnixStream, rx: Receiver<Arc<Vec<u8>>>) {
    for bytes in rx {
        if stream.write_all(&bytes).is_err() {
            break;
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

fn answer(id: u64, result: Result<(), String>) -> ServerMessage {
    match result {
        Ok(()) => ServerMessage::Reply(Reply {
            id,
            ok: true,
            body: ReplyBody::Empty {},
        }),
        Err(e) => ServerMessage::Error(ErrorMessage::new(Some(id), code::BAD_REQUEST, e)),
    }
}

/// Export and replay files are read here, outside the server's lock.
fn serve(core: &Mutex<Core>, client: u64, mut stream: UnixStream) {
    while let Ok(Some(f)) = frame::read(&mut stream) {
        if f.kind != Kind::Json {
            return;
        }
        let request = match parse_request(&f.payload) {
            Ok(r) => r,
            Err(e) => {
                core.lock().unwrap().send(client, &ServerMessage::Error(e));
                continue;
            }
        };
        let id = request.id;
        let hello = core.lock().unwrap().said_hello(client);
        match request.op {
            ClientOp::Bye => return,
            ClientOp::Export { dir, out, metrics } if hello => {
                let result = export::export(Path::new(&dir), Path::new(&out), &metrics);
                let message = answer(id, result.map(|_| ()));
                core.lock().unwrap().send(client, &message);
            }
            ClientOp::ReplayOpen { dir } if hello => {
                let Some(tx) = core.lock().unwrap().sender(client) else {
                    return;
                };
                let send = Box::new(move |m: &ServerMessage| {
                    tx.try_send(Arc::new(frame::encode_json(m))).is_ok()
                });
                let opened = Replay::open(Path::new(&dir), send);
                let mut core = core.lock().unwrap();
                match opened {
                    Ok(replay) => core.replay_add(client, id, replay),
                    Err(e) => core.send(client, &answer(id, Err(format!("{dir}: {e}")))),
                }
            }
            op => {
                if !core
                    .lock()
                    .unwrap()
                    .handle(client, ClientRequest { id, op })
                {
                    return;
                }
            }
        }
    }
}
