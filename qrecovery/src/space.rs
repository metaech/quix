use super::{
    crypto::{CryptoStream, TransmitCrypto},
    rcvdpkt::{ArcRcvdPktRecords, Error as RcvPnError},
    reliable::{ArcReliableFrameQueue, ArcSentPktRecords, SentRecord},
    streams::{none::NoDataStreams, ArcDataStreams, ReceiveStream, TransmitStream},
};
use bytes::{BufMut, Bytes};
use qbase::{
    error::Error,
    frame::{
        io::{WriteAckFrame, WriteFrame},
        AckFrame, BeFrame, DataFrame, StreamCtlFrame,
    },
    packet::{PacketNumber, WritePacketNumber},
    streamid::Role,
};
use std::{fmt::Debug, sync::Arc, time::Instant};

#[derive(Debug, Clone)]
pub enum SpaceFrame {
    Stream(StreamCtlFrame),
    Data(DataFrame, Bytes),
}

#[derive(Debug)]
struct RawSpace<T> {
    reliable_frame_queue: ArcReliableFrameQueue,
    sent_pkt_records: ArcSentPktRecords,
    rcvd_pkt_records: ArcRcvdPktRecords,
    data_streams: T,
    crypto_stream: CryptoStream,
}

impl<T> RawSpace<T>
where
    T: TransmitStream + ReceiveStream,
{
    fn rcvd_pkt_records(&self) -> ArcRcvdPktRecords {
        self.rcvd_pkt_records.clone()
    }

    fn decode_pn(&self, encoded_pn: PacketNumber) -> Result<u64, RcvPnError> {
        self.rcvd_pkt_records.decode_pn(encoded_pn)
    }

    fn on_rcvd_pn(&self, pn: u64) {
        self.rcvd_pkt_records.on_rcvd_pn(pn)
    }

    fn read(&self, mut buf: &mut [u8], ack_pkt: Option<(u64, Instant)>) -> (u64, usize, usize) {
        let origin = buf.remaining_mut();

        let mut send_guard = self.sent_pkt_records.send();
        let (pn, encoded_pn) = send_guard.next_pn();
        if buf.remaining_mut() > encoded_pn.size() {
            buf.put_packet_number(encoded_pn);
        } else {
            return (pn, encoded_pn.size(), 0);
        }

        if let Some(largest) = ack_pkt {
            let ack_frame = self
                .rcvd_pkt_records
                .gen_ack_frame_util(largest, buf.remaining_mut());
            buf.put_ack_frame(&ack_frame);
            send_guard.record_ack_frame(ack_frame);
        }

        {
            let mut read_frame_guard = self.reliable_frame_queue.read();
            while let Some(frame) = read_frame_guard.front() {
                let remaining = buf.remaining_mut();
                if remaining > frame.max_encoding_size() || remaining > frame.encoding_size() {
                    buf.put_frame(frame);
                    let frame = read_frame_guard.pop_front().unwrap();
                    send_guard.record_reliable_frame(frame);
                } else {
                    break;
                }
            }
        }

        // 尝试写入流数据，优先写入加密流数据，然后再努力写入数据流数据
        if let Some((crypto_frame, len)) = self.crypto_stream.try_read_data(buf) {
            send_guard.record_data_frame(DataFrame::Crypto(crypto_frame));
            unsafe {
                buf.advance_mut(len);
            }
        }
        if let Some((stream_frame, len)) = self.data_streams.try_read_data(buf) {
            send_guard.record_data_frame(DataFrame::Stream(stream_frame));
            unsafe {
                buf.advance_mut(len);
            }
        }

        (pn, encoded_pn.size(), origin - buf.remaining_mut())
    }

    fn receive(&self, frame: SpaceFrame) -> Result<(), Error> {
        match frame {
            SpaceFrame::Stream(frame) => {
                self.data_streams.recv_frame(frame)?;
            }
            SpaceFrame::Data(DataFrame::Crypto(frame), data) => {
                self.crypto_stream.recv_data(frame, data)?;
            }
            SpaceFrame::Data(DataFrame::Stream(frame), data) => {
                self.data_streams.recv_data(frame, data)?;
            }
        }
        Ok(())
    }

    fn on_ack(&self, ack: AckFrame) {
        let mut recv_guard = self.sent_pkt_records.receive();
        recv_guard.update_largest(ack.largest.into_inner());

        for pn in ack.iter().flat_map(|r| r.rev()) {
            for record in recv_guard.on_pkt_acked(pn) {
                match record {
                    SentRecord::Ack(_) => {
                        // do nothing
                    }
                    SentRecord::Reliable(_) => {
                        // do nothing
                    }
                    SentRecord::Data(DataFrame::Crypto(frame)) => {
                        self.crypto_stream.on_data_acked(frame);
                    }
                    SentRecord::Data(DataFrame::Stream(frame)) => {
                        self.data_streams.on_data_acked(frame);
                    }
                }
            }
        }
    }

    fn may_loss_pkt(&self, pn: u64) {
        let mut recv_pkt_guard = self.sent_pkt_records.receive();
        let mut write_frame_guard = self.reliable_frame_queue.write();
        for record in recv_pkt_guard.may_loss_pkt(pn) {
            match record {
                SentRecord::Ack(_) => {
                    // do nothing
                }
                SentRecord::Reliable(frame) => {
                    write_frame_guard.push_reliable_frame(frame);
                }
                SentRecord::Data(DataFrame::Crypto(frame)) => {
                    self.crypto_stream.may_loss_data(frame);
                }
                SentRecord::Data(DataFrame::Stream(frame)) => {
                    self.data_streams.may_loss_data(frame);
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArcSpace<T>(Arc<RawSpace<T>>);

impl<T> ArcSpace<T>
where
    T: TransmitStream + ReceiveStream,
{
    /// 可用于收包解码包号，判定包号是否重复或者过期，记录收包状态，淘汰并滑动收包记录窗口
    pub fn rcvd_pkt_records(&self) -> ArcRcvdPktRecords {
        self.0.rcvd_pkt_records()
    }

    pub fn decode_pn(&self, encoded_pn: PacketNumber) -> Result<u64, RcvPnError> {
        self.0.decode_pn(encoded_pn)
    }

    pub fn on_rcvd_pn(&self, pn: u64) {
        self.0.on_rcvd_pn(pn)
    }

    /// 要发送一个该空间的数据包，读出下一个包号，然后检车是否要发送AckFrame，
    /// 然后发送帧，最后发送数据流中的数据帧。
    /// 返回该数据包的包号，以及大小
    pub fn read(&self, buf: &mut [u8], ack_pkt: Option<(u64, Instant)>) -> (u64, usize, usize) {
        self.0.read(buf, ack_pkt)
    }

    /// 接收Space相关的帧，包括数据帧
    pub fn receive(&self, frame: SpaceFrame) -> Result<(), Error> {
        self.0.receive(frame)
    }

    /// 此处接收AckFrame，只负责内容，涉及RTT和传输速度控制的，path已经处理过
    pub fn on_ack(&self, ack: AckFrame) {
        self.0.on_ack(ack);
    }

    /// 当数据包在传输中丢失，通常由Path判断，通过某种通信方式告知Space，并调用该函数
    pub fn may_loss_pkt(&self, pn: u64) {
        self.0.may_loss_pkt(pn);
    }
}

impl ArcSpace<NoDataStreams> {
    /// Initial空间和Handshake空间皆通过此函数创建
    pub fn with_crypto_stream(crypto_stream: CryptoStream) -> Self {
        ArcSpace(Arc::new(RawSpace {
            reliable_frame_queue: Default::default(),
            sent_pkt_records: Default::default(),
            rcvd_pkt_records: Default::default(),
            data_streams: NoDataStreams,
            crypto_stream,
        }))
    }
}

impl ArcSpace<ArcDataStreams> {
    /// 数据空间通过此函数创建
    pub fn new(
        role: Role,
        max_bi_streams: u64,
        max_uni_streams: u64,
        crypto_stream: CryptoStream,
    ) -> Self {
        let reliable_frame_queue = ArcReliableFrameQueue::default();
        ArcSpace(Arc::new(RawSpace {
            reliable_frame_queue: reliable_frame_queue.clone(),
            sent_pkt_records: Default::default(),
            rcvd_pkt_records: Default::default(),
            data_streams: ArcDataStreams::with_role_and_limit(
                role,
                max_bi_streams,
                max_uni_streams,
                reliable_frame_queue,
            ),
            crypto_stream,
        }))
    }

    pub fn data_streams(&self) -> ArcDataStreams {
        self.0.data_streams.clone()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
