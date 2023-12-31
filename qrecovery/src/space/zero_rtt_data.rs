/// Application data space, 0-RTT data space
use super::{one_rtt_data, OneRttDataSpace, Receive};
use crate::{crypto_stream::CryptoStream, rtt::Rtt, streams::Streams};
use qbase::{
    error::Error,
    frame::{ConnectionFrame, OneRttFrame, StreamFrame, StreamInfoFrame, ZeroRttFrame},
    packet::{
        decrypt::{DecodeHeader, DecryptPacket, RemoteProtection},
        PacketWrapper, ZeroRttHeader,
    },
    streamid::StreamIds,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::{
    select,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

type ZeroRttDataFrame = StreamFrame;
pub type ZeroRttDataSpace = super::Space<ZeroRttFrame, ZeroRttDataFrame, Transmission, false>;

#[derive(Debug)]
pub struct Transmission {
    streams: Streams,
    close_tx: oneshot::Sender<()>,
    join_handler: JoinHandle<mpsc::UnboundedReceiver<StreamInfoFrame>>,
}

impl super::Transmit<ZeroRttFrame, ZeroRttDataFrame> for Transmission {
    type Buffer = Vec<u8>;

    fn try_send(&mut self, buf: &mut Self::Buffer) -> Option<(ZeroRttDataFrame, usize)> {
        self.streams.try_send(buf)
    }

    fn confirm_data(&mut self, stream_frame: ZeroRttDataFrame) {
        self.streams.confirm_data(stream_frame);
    }

    fn may_loss(&mut self, stream_frame: ZeroRttDataFrame) {
        self.streams.may_loss(stream_frame);
    }

    fn recv_data(
        &mut self,
        stream_frame: ZeroRttDataFrame,
        body: bytes::Bytes,
    ) -> Result<(), Error> {
        self.streams.recv_data(stream_frame, body)
    }

    fn recv_frame(&mut self, frame: ZeroRttFrame) -> Result<Option<ConnectionFrame>, Error> {
        match frame {
            ZeroRttFrame::Stream(frame) => self.streams.recv_frame(frame),
            ZeroRttFrame::MaxData(frame) => Ok(Some(frame.into())),
            ZeroRttFrame::DataBlocked(frame) => Ok(Some(frame.into())),
            ZeroRttFrame::NewConnectionId(frame) => Ok(Some(frame.into())),
            ZeroRttFrame::RetireConnectionId(frame) => Ok(Some(frame.into())),
            ZeroRttFrame::PathChallenge(frame) => Ok(Some(frame.into())),
            ZeroRttFrame::Ping(_) => unreachable!("these are handled in connection layer"),
        }
    }
}

impl Transmission {
    pub fn streams(&mut self) -> &mut Streams {
        &mut self.streams
    }
}

impl ZeroRttDataSpace {
    pub fn new(stream_ids: StreamIds) -> Self {
        let frames = Arc::new(Mutex::new(VecDeque::new()));
        let (frame_tx, mut frame_rx) = tokio::sync::mpsc::unbounded_channel();
        let (close_tx, mut close_rx) = tokio::sync::oneshot::channel::<()>();
        let join_handler = tokio::spawn({
            let frames = frames.clone();
            async move {
                loop {
                    select! {
                        _ = &mut close_rx => break,
                        frame = frame_rx.recv() => {
                            if let Some(f) = frame {
                                frames.lock().unwrap().push_back(ZeroRttFrame::Stream(f));
                            } else {
                                break;
                            }
                        }
                    }
                }
                frame_rx
            }
        });
        let streams = Streams::new(stream_ids, frame_tx);
        let transmission = Transmission {
            streams,
            close_tx,
            join_handler,
        };
        ZeroRttDataSpace::build(frames, transmission)
    }

    pub async fn upgrade(self, crypto_stream: CryptoStream) -> OneRttDataSpace {
        let frames = Arc::new(Mutex::new(VecDeque::new()));
        self.transmission.close_tx.send(()).unwrap();
        let mut stream_info_frame_rx = self.transmission.join_handler.await.unwrap();
        frames
            .lock()
            .unwrap()
            .extend(self.frames.lock().unwrap().drain(..).map(OneRttFrame::from));
        tokio::spawn({
            let frames = frames.clone();
            async move {
                while let Some(frame) = stream_info_frame_rx.recv().await {
                    frames.lock().unwrap().push_back(OneRttFrame::Stream(frame));
                }
            }
        });
        let transmission =
            one_rtt_data::Transmission::new(self.transmission.streams, crypto_stream);
        OneRttDataSpace::build(frames, transmission)
    }
}

impl super::ReceivePacket for super::ReceiveHalf<ZeroRttDataSpace> {
    type Packet = PacketWrapper<ZeroRttHeader>;

    fn receive_packet(
        &self,
        mut packet: Self::Packet,
        rtt: &mut Rtt,
    ) -> Result<Vec<ConnectionFrame>, Error> {
        let mut space = self.space.lock().unwrap();

        let ok = packet.remove_protection(&self.decrypt_keys.header);
        if ok {
            let pn = packet.decode_header()?;
            let (pktid, payload) = packet
                .decrypt_packet(pn, space.expected_pn(), &self.decrypt_keys.packet)
                .unwrap();
            space.receive(pktid, payload, rtt)
        } else {
            todo!()
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
