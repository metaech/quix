use crate::{
    crypto::{CryptoStream, TransmitCrypto},
    index_deque::IndexDeque,
    rtt::Rtt,
    streams::{data::ArcOutput, TransmitStream},
};
use bytes::BufMut;
use qbase::{
    frame::{
        io::WriteFrame, AckFrame, AckRecord, BeFrame, ConnFrame, DataFrame, ReliableFrame,
        StreamCtlFrame,
    },
    packet::PacketNumber,
    varint::VARINT_MAX,
    SpaceId,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::ObserveAck;

#[derive(Debug, Clone)]
pub enum Record {
    Reliable(ReliableFrame),
    Data(DataFrame),
    Ack(AckRecord),
}

pub type Payload = Vec<Record>;

#[derive(Debug, Clone)]
pub struct Packet {
    pub send_time: Instant,
    pub payload: Payload,
    pub sent_bytes: usize,
    pub is_ack_eliciting: bool,
}

#[derive(Debug)]
struct Transmitter<TX, O> {
    space_id: SpaceId,
    // 以下三个字段，是为了重传需要，也为了发送数据需要
    sending_frames: Arc<Mutex<VecDeque<ReliableFrame>>>,
    // 其实只需要CryptoString的Outgoing部分
    crypto_stream: CryptoStream,
    // 其实只需要TransmitStream的Outgoing部分
    inner: TX,

    // 正在飞行中的数据包记录，如果某个包为None，说明已经确认到达了
    inflight_packets: IndexDeque<Option<Packet>, VARINT_MAX>,
    // 上一次发送ack-eliciting包的时间
    time_of_last_sent_ack_eliciting_packet: Option<Instant>,
    // 对方确认的最大包id，可认为对方收到的最大包id，尽管可能有半rtt以上时间的信息过时
    largest_acked_pktid: Option<u64>,
    // ack被ack的消息对接收很重要，需要通知接收端
    ack_observer: O,
}

impl<TX: TransmitStream, O: ObserveAck> Transmitter<TX, O> {
    fn new(
        space_id: SpaceId,
        crypto_stream: CryptoStream,
        inner: TX,
        sending_frames: Arc<Mutex<VecDeque<ReliableFrame>>>,
        ack_observer: O,
    ) -> Self {
        Self {
            space_id,
            sending_frames,
            crypto_stream,
            inner,
            inflight_packets: IndexDeque::default(),
            time_of_last_sent_ack_eliciting_packet: None,
            largest_acked_pktid: None,
            ack_observer,
        }
    }

    fn next_pkt_no(&self) -> (u64, PacketNumber) {
        let pkt_id = self.inflight_packets.largest();
        let pn = PacketNumber::encode(pkt_id, self.largest_acked_pktid.unwrap_or(0));
        (pkt_id, pn)
    }

    fn read(&mut self, mut buf: &mut [u8]) -> usize {
        let mut is_ack_eliciting = false;
        let remaning = buf.remaining_mut();

        let mut records = Payload::new();
        {
            // Prioritize retransmitting lost or info frames.
            let mut frames = self.sending_frames.lock().unwrap();
            while let Some(frame) = frames.front() {
                if buf.remaining_mut() >= frame.max_encoding_size()
                    || buf.remaining_mut() >= frame.encoding_size()
                {
                    buf.put_frame(frame);
                    is_ack_eliciting = true;

                    let frame = frames.pop_front().unwrap();
                    records.push(Record::Reliable(frame));
                } else {
                    break;
                }
            }
        }

        // Consider transmitting data frames.
        if self.space_id != SpaceId::ZeroRtt {
            while let Some((data_frame, len)) = self.crypto_stream.try_read_data(buf) {
                records.push(Record::Data(DataFrame::Crypto(data_frame)));
                unsafe {
                    buf.advance_mut(len);
                }
            }
        }
        while let Some((data_frame, len)) = self.inner.try_read_data(buf) {
            records.push(Record::Data(DataFrame::Stream(data_frame)));
            unsafe {
                buf.advance_mut(len);
            }
        }

        // Record
        let sent_bytes = remaning - buf.remaining_mut();
        if sent_bytes == 0 {
            // no data to send
            return 0;
        }
        if is_ack_eliciting {
            self.time_of_last_sent_ack_eliciting_packet = Some(Instant::now());
        }
        self.record_sent_packet(Packet {
            send_time: Instant::now(),
            payload: records,
            sent_bytes,
            is_ack_eliciting,
        });
        sent_bytes
    }

    /// 单单发送一个AckFrame，也要记录
    fn record_sent_packet(&mut self, packet: Packet) {
        let _pkt_id = self.inflight_packets.push(Some(packet)).expect(
            r#"The packet number cannot exceed 2^62. Even if 100 million packets are sent 
                per second, it would take more than a million years to exceed this limit."#,
        );
    }

    fn recv_ack_frame(&mut self, mut ack: AckFrame, rtt: Arc<Mutex<Rtt>>) {
        let largest_acked = ack.largest.into_inner();
        if self
            .largest_acked_pktid
            .map(|v| v > largest_acked)
            .unwrap_or(false)
        {
            return;
        }
        // largest_acked == self.largest_acked_packet is also acceptable,
        // perhaps indicating that old 'lost' packets have been acknowledged.
        self.largest_acked_pktid = Some(largest_acked);

        let mut no_newly_acked = true;
        let mut includes_ack_eliciting = false;
        let mut rtt_sample = None;
        let ecn_in_ack = ack.take_ecn();
        let ack_delay = Duration::from_micros(ack.delay.into_inner());
        for range in ack.iter() {
            for pktid in range {
                if let Some(packet) = self
                    .inflight_packets
                    .get_mut(pktid)
                    .and_then(|record| record.take())
                {
                    no_newly_acked = false;
                    if packet.is_ack_eliciting {
                        includes_ack_eliciting = true;
                    }
                    if pktid == largest_acked {
                        rtt_sample = Some(packet.send_time.elapsed());
                    }
                    self.confirm_packet_rcvd(pktid, packet);
                }
            }
        }

        if no_newly_acked {
            return;
        }

        if includes_ack_eliciting {
            let is_handshake_confirmed = self.space_id == SpaceId::OneRtt;
            if let Some(latest_rtt) = rtt_sample {
                rtt.lock()
                    .unwrap()
                    .update(latest_rtt, ack_delay, is_handshake_confirmed);
            }
        }

        if let Some(_ecn) = ecn_in_ack {
            todo!("处理ECN信息");
        }
    }

    /// A small optimization would be to slide forward if the first consecutive
    /// packets in the inflight_packets queue have been acknowledged.
    fn slide_inflight_pkt_window(&mut self) {
        let n = self
            .inflight_packets
            .iter()
            .take_while(|p| p.is_none())
            .count();
        self.inflight_packets.advance(n);
    }

    fn confirm_packet_rcvd(&mut self, _pkt_id: u64, packet: Packet) {
        // TODO: 此处应通知发送该包的路径，让该路径计算速度，并最终用于判定那些包丢了

        // 告知有关模块该包负载的内容已被接收
        for record in packet.payload {
            match record {
                Record::Ack(ack) => {
                    // TODO:
                    self.ack_observer.on_ack_rcvd(ack.0);
                }
                Record::Reliable(frame) => {
                    if let ReliableFrame::Stream(StreamCtlFrame::ResetStream(f)) = frame {
                        self.inner.confirm_reset_rcvd(f);
                    }
                }
                Record::Data(data) => match data {
                    DataFrame::Crypto(f) => self.crypto_stream.confirm_data_rcvd(f),
                    DataFrame::Stream(f) => self.inner.confirm_data_rcvd(f),
                },
            }
        }
    }

    fn may_loss_packet(&mut self, pkt_id: u64) {
        // retranmit the frames, tell stream that the data is lost and need to retranmit in future
        if let Some(packet) = self
            .inflight_packets
            .get_mut(pkt_id)
            .and_then(|record| record.take())
        {
            for record in packet.payload {
                match record {
                    Record::Ack(_) => { /* needn't resend */ }
                    Record::Reliable(frame) => {
                        let mut frames = self.sending_frames.lock().unwrap();
                        frames.push_back(frame);
                    }
                    Record::Data(data) => match data {
                        DataFrame::Crypto(f) => self.crypto_stream.may_loss_data(f),
                        DataFrame::Stream(f) => self.inner.may_loss_data(f),
                    },
                }
            }
        }
    }
}

impl<O: ObserveAck> Transmitter<ArcOutput, O> {
    fn upgrade(&mut self) {
        self.space_id = SpaceId::OneRtt;
    }

    fn write_conn_frame(&self, frame: ConnFrame) {
        assert!(frame.belongs_to(self.space_id));
        let mut frames = self.sending_frames.lock().unwrap();
        frames.push_back(ReliableFrame::Conn(frame));
    }

    fn write_stream_frame(&self, frame: StreamCtlFrame) {
        assert!(frame.belongs_to(self.space_id));
        let mut frames = self.sending_frames.lock().unwrap();
        frames.push_back(ReliableFrame::Stream(frame));
    }
}

#[derive(Clone, Debug)]
pub struct ArcTransmitter<TX, O> {
    inner: Arc<Mutex<Transmitter<TX, O>>>,
}

impl<ST, O> ArcTransmitter<ST, O>
where
    ST: TransmitStream + Send + 'static,
    O: ObserveAck + Send + 'static,
{
    /// 一个Transmitter，不仅仅要发送数据，还要接收AckFrame，以及丢包序号去重传。
    /// 然后，接收端提供的AckFrame如果被确认了，也需要通知到接收端
    pub fn new(
        space_id: SpaceId,
        crypto_stream: CryptoStream,
        sending_frames: Arc<Mutex<VecDeque<ReliableFrame>>>,
        inner: ST,
        ack_observer: O,
    ) -> Self {
        let transmitter = Arc::new(Mutex::new(Transmitter::new(
            space_id,
            crypto_stream,
            inner,
            sending_frames,
            ack_observer,
        )));
        Self { inner: transmitter }
    }
}

impl<ST: TransmitStream, O: ObserveAck> ArcTransmitter<ST, O> {
    pub fn next_pkt_no(&self) -> (u64, PacketNumber) {
        self.inner.lock().unwrap().next_pkt_no()
    }

    pub fn read(&self, buf: &mut [u8]) -> usize {
        self.inner.lock().unwrap().read(buf)
    }

    pub fn record_sent_ack(&self, packet: Packet) {
        self.inner.lock().unwrap().record_sent_packet(packet);
    }

    pub fn recv_ack_frame(&self, ack: AckFrame, rtt: Arc<Mutex<Rtt>>) {
        self.inner.lock().unwrap().recv_ack_frame(ack, rtt);
    }

    pub fn may_loss_packet(&self, pkt_id: u64) {
        let mut guard = self.inner.lock().unwrap();
        guard.may_loss_packet(pkt_id);
        guard.slide_inflight_pkt_window();
    }
}

impl<O: ObserveAck> ArcTransmitter<ArcOutput, O> {
    pub fn upgrade(&self) {
        self.inner.lock().unwrap().upgrade();
    }

    pub fn write_conn_frame(&self, frame: ConnFrame) {
        self.inner.lock().unwrap().write_conn_frame(frame);
    }

    pub fn write_stream_frame(&self, frame: StreamCtlFrame) {
        self.inner.lock().unwrap().write_stream_frame(frame);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
