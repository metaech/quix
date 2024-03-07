use super::{
    crypto::CryptoStream,
    rtt::Rtt,
    streams::{none::NoDataStreams, ArcDataStreams, Output, ReceiveStream, TransmitStream},
};
use bytes::Bytes;
use qbase::{frame::*, packet::PacketNumber, util::ArcAsyncQueue, SpaceId};
use std::{
    collections::VecDeque,
    fmt::Debug,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::{self, UnboundedReceiver};

pub mod rx;
pub mod tx;

#[derive(Debug, Clone)]
pub enum SpaceFrame {
    Ack(AckFrame, Arc<Mutex<Rtt>>),
    Stream(StreamCtlFrame),
    Data(DataFrame, Bytes),
}

pub trait TransmitPacket {
    fn next_pkt_no(&self) -> (u64, PacketNumber);

    fn read(&self, buf: &mut [u8]) -> usize;
}

/// When a network socket receives a data packet and determines that it belongs
/// to a specific space, the content of the packet is passed on to that space.
pub trait ReceivePacket {
    fn receive_pkt_no(&self, pn: PacketNumber) -> (u64, bool);

    fn record(&self, pktid: u64, is_ack_eliciting: bool);
}

/// 可靠空间的抽象实现，需要实现上述所有trait
/// 可靠空间中的重传、确认，由可靠空间内部实现，无需外露
#[derive(Debug)]
struct Space<S: Output + Debug> {
    transmitter: tx::ArcTransmitter<<S as Output>::Outgoing>,
    receiver: rx::ArcReceiver<S>,
}

impl<S> Space<S>
where
    S: Debug + Output + ReceiveStream + Clone + Send + 'static,
    <S as Output>::Outgoing: TransmitStream + Clone + Send + 'static,
{
    pub(crate) fn build(
        space_id: SpaceId,
        crypto_stream: CryptoStream,
        sending_frames: Arc<Mutex<VecDeque<ReliableFrame>>>,
        data_stream: S,
        ack_frame_rx: UnboundedReceiver<(AckFrame, Arc<Mutex<Rtt>>)>,
        loss_pkt_rx: UnboundedReceiver<u64>,
        recv_frames_queue: ArcAsyncQueue<SpaceFrame>,
    ) -> Self {
        let (ack_record_tx, ack_record_rx) = mpsc::unbounded_channel();
        let transmitter = tx::ArcTransmitter::new(
            space_id,
            crypto_stream.clone(),
            sending_frames,
            data_stream.output(),
            ack_record_tx,
            ack_frame_rx,
            loss_pkt_rx,
        );
        let receiver = rx::ArcReceiver::new(
            space_id,
            crypto_stream,
            data_stream,
            recv_frames_queue,
            ack_record_rx,
        );
        Self {
            transmitter,
            receiver,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArcSpace<S: Debug + Output>(Arc<Space<S>>);

impl ArcSpace<NoDataStreams> {
    pub fn new_initial_space(
        crypto_stream: CryptoStream,
        ack_frame_rx: UnboundedReceiver<(AckFrame, Arc<Mutex<Rtt>>)>,
        loss_pkt_rx: UnboundedReceiver<u64>,
        recv_frames_queue: ArcAsyncQueue<SpaceFrame>,
    ) -> Self {
        Self(Arc::new(Space::build(
            SpaceId::Initial,
            crypto_stream,
            Arc::new(Mutex::new(VecDeque::new())),
            NoDataStreams,
            ack_frame_rx,
            loss_pkt_rx,
            recv_frames_queue,
        )))
    }

    pub fn new_handshake_space(
        crypto_stream: CryptoStream,
        ack_frame_rx: UnboundedReceiver<(AckFrame, Arc<Mutex<Rtt>>)>,
        loss_pkt_rx: UnboundedReceiver<u64>,
        recv_frames_queue: ArcAsyncQueue<SpaceFrame>,
    ) -> Self {
        Self(Arc::new(Space::build(
            SpaceId::Handshake,
            crypto_stream,
            Arc::new(Mutex::new(VecDeque::new())),
            NoDataStreams,
            ack_frame_rx,
            loss_pkt_rx,
            recv_frames_queue,
        )))
    }
}

/// Data space, initially it's a 0RTT space, and later it needs to be upgraded to a 1RTT space.
/// The data in the 0RTT space is unreliable and cannot transmit CryptoFrame. It is constrained
/// by the space_id when sending, and a judgment is also made in the task of receiving and unpacking.
/// Therefore, when upgrading, just change the space_id to 1RTT, no other operations are needed.
impl ArcSpace<ArcDataStreams> {
    pub fn new_data_space(
        crypto_stream: CryptoStream,
        streams: ArcDataStreams,
        sending_frames: Arc<Mutex<VecDeque<ReliableFrame>>>,
        ack_frame_rx: UnboundedReceiver<(AckFrame, Arc<Mutex<Rtt>>)>,
        loss_pkt_rx: UnboundedReceiver<u64>,
        recv_frames_queue: ArcAsyncQueue<SpaceFrame>,
    ) -> Self {
        Self(Arc::new(Space::build(
            SpaceId::ZeroRtt,
            crypto_stream,
            sending_frames,
            streams,
            ack_frame_rx,
            loss_pkt_rx,
            recv_frames_queue,
        )))
    }

    pub fn upgrade(&self) {
        self.0.transmitter.upgrade();
        self.0.receiver.upgrade();
    }

    /*
    pub fn write_conn_frame(&self, frame: ConnFrame) {
        self.0.transmitter.write_conn_frame(frame);
    }

    pub fn write_stream_frame(&self, frame: StreamCtlFrame) {
        self.0.transmitter.write_stream_frame(frame);
    }
    */
}

impl<S> ReceivePacket for ArcSpace<S>
where
    S: Debug + ReceiveStream + Output,
{
    fn receive_pkt_no(&self, pn: PacketNumber) -> (u64, bool) {
        self.0.receiver.receive_pkt_no(pn)
    }

    fn record(&self, pkt_id: u64, is_ack_eliciting: bool) {
        self.0.receiver.record(pkt_id, is_ack_eliciting);
    }
}

impl<S> TransmitPacket for ArcSpace<S>
where
    S: Debug + Output,
    <S as Output>::Outgoing: TransmitStream,
{
    /// Get the next packet number. This number is not thread-safe.
    /// It does not lock the next packet number to be sent.
    /// Before it is actually sent, other transmiting threads/tasks may get the
    /// same next packet number, causing conflicts. Therefore, it is required
    /// that there should only be one sending thread/task for a connection.
    fn next_pkt_no(&self) -> (u64, PacketNumber) {
        self.0.transmitter.next_pkt_no()
    }

    /// Read the data to be sent and put it into the buffer.
    /// Returns the actual number of bytes read. If it is 0,
    /// it means there is no suitable data to send.
    fn read(&self, buf: &mut [u8]) -> usize {
        self.0.transmitter.read(buf)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4);
    }
}
