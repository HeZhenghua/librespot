use byteorder::{BigEndian, ByteOrder, ReadBytesExt, WriteBytesExt};
use eventual;
use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::mem;

use util::{SpotifyId, FileId};
use session::Session;
use connection::PacketHandler;

pub type AudioKey = [u8; 16];
#[derive(Debug,Hash,PartialEq,Eq,Copy,Clone)]
pub struct AudioKeyError;

#[derive(Debug,Hash,PartialEq,Eq,Clone)]
struct AudioKeyId(SpotifyId, FileId);

enum AudioKeyStatus {
    Loading(Vec<eventual::Complete<AudioKey, AudioKeyError>>),
    Loaded(AudioKey),
    Failed(AudioKeyError),
}

pub struct AudioKeyManager {
    next_seq: u32,
    pending: HashMap<u32, AudioKeyId>,
    cache: HashMap<AudioKeyId, AudioKeyStatus>,
}

impl AudioKeyManager {
    pub fn new() -> AudioKeyManager {
        AudioKeyManager {
            next_seq: 1,
            pending: HashMap::new(),
            cache: HashMap::new(),
        }
    }

    fn send_key_request(&mut self, session: &Session, track: SpotifyId, file: FileId) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;

        let mut data: Vec<u8> = Vec::new();
        data.write(&file.0).unwrap();
        data.write(&track.to_raw()).unwrap();
        data.write_u32::<BigEndian>(seq).unwrap();
        data.write_u16::<BigEndian>(0x0000).unwrap();

        session.send_packet(0xc, &data).unwrap();

        seq
    }

    pub fn request(&mut self,
                   session: &Session,
                   track: SpotifyId,
                   file: FileId)
                   -> eventual::Future<AudioKey, AudioKeyError> {

        let id = AudioKeyId(track, file);
        self.cache
            .get_mut(&id)
            .map(|status| {
                match *status {
                    AudioKeyStatus::Failed(error) => eventual::Future::error(error),
                    AudioKeyStatus::Loaded(key) => eventual::Future::of(key),
                    AudioKeyStatus::Loading(ref mut req) => {
                        let (tx, rx) = eventual::Future::pair();
                        req.push(tx);
                        rx
                    }
                }
            })
            .unwrap_or_else(|| {
                let seq = self.send_key_request(session, track, file);
                self.pending.insert(seq, id.clone());

                let (tx, rx) = eventual::Future::pair();
                self.cache.insert(id, AudioKeyStatus::Loading(vec![tx]));
                rx
            })
    }
}

impl PacketHandler for AudioKeyManager {
    fn handle(&mut self, cmd: u8, data: Vec<u8>) {
        let mut data = Cursor::new(data);
        let seq = data.read_u32::<BigEndian>().unwrap();

        if let Some(status) = self.pending.remove(&seq).and_then(|id| self.cache.get_mut(&id)) {
            if cmd == 0xd {
                let mut key = [0u8; 16];
                data.read_exact(&mut key).unwrap();

                let status = mem::replace(status, AudioKeyStatus::Loaded(key));

                if let AudioKeyStatus::Loading(cbs) = status {
                    for cb in cbs {
                        cb.complete(key);
                    }
                }
            } else if cmd == 0xe {
                let error = AudioKeyError;
                let status = mem::replace(status, AudioKeyStatus::Failed(error));

                if let AudioKeyStatus::Loading(cbs) = status {
                    for cb in cbs {
                        cb.fail(error);
                    }
                }
            }
        }
    }
}
