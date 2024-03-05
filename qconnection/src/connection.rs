use crate::{auto, crypto::TlsIO, handshake, path::ArcPath};
use qbase::{
    packet::{
        keys::{ArcKeys, ArcOneRttKeys},
        HandshakePacket, InitialPacket, OneRttPacket, SpinBit, ZeroRttPacket,
    },
    streamid::Role,
    SpaceId,
};
use qrecovery::{
    crypto::CryptoStream,
    frame_queue::ArcFrameQueue,
    space::ArcSpace,
    streams::{data::DataStreams, none::NoDataStreams},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

/// Option是为了能丢弃前期空间，包括这些空间的收包队列，
/// 一旦丢弃，后续再收到该空间的包，直接丢弃。
type RxPacketsQueue<T> = Option<mpsc::UnboundedSender<(T, ArcPath)>>;

pub struct Connection {
    // Thus, a client MUST discard Initial keys when it first sends a Handshake packet
    // and a server MUST discard Initial keys when it first successfully processes a
    // Handshake packet. Endpoints MUST NOT send Initial packets after this point.
    initial_keys: ArcKeys,
    initial_pkt_queue: RxPacketsQueue<InitialPacket>,
    // 发送数据，也可以随着升级到Handshake空间而丢弃
    initial_space: ArcSpace<NoDataStreams>,

    // An endpoint MUST discard its Handshake keys when the TLS handshake is confirmed.
    handshake_keys: ArcKeys,
    handshake_pkt_queue: RxPacketsQueue<HandshakePacket>,
    // 发送数据，也可以随着升级到1RTT空间而丢弃
    handshake_space: ArcSpace<NoDataStreams>,

    zero_rtt_keys: ArcKeys,
    // 发送数据，也可以随着升级到1RTT空间而丢弃
    zero_rtt_pkt_queue: RxPacketsQueue<ZeroRttPacket>,
    one_rtt_pkt_queue: mpsc::UnboundedSender<(OneRttPacket, ArcPath)>,
    data_space: ArcSpace<DataStreams>,
    spin: SpinBit,
}

impl Connection {
    pub fn new(tls_session: TlsIO) -> Self {
        let rcvd_conn_frames = ArcFrameQueue::new();

        let (initial_pkt_tx, initial_pkt_rx) =
            mpsc::unbounded_channel::<(InitialPacket, ArcPath)>();
        let (initial_ack_tx, initial_ack_rx) = mpsc::unbounded_channel();
        let (initial_loss_tx, initial_loss_rx) = mpsc::unbounded_channel();
        let initial_crypto_stream = CryptoStream::new(1000_000, 1000_000);
        let initial_crypto_handler = initial_crypto_stream.split();
        let initial_keys = ArcKeys::new_pending();
        let initial_space_frame_queue = ArcFrameQueue::new();
        let initial_space = ArcSpace::new_initial_space(
            initial_crypto_stream,
            initial_ack_rx,
            initial_loss_rx,
            initial_space_frame_queue.clone(),
        );
        tokio::spawn(
            auto::loop_read_long_packet_and_then_dispatch_to_space_frame_queue(
                initial_pkt_rx,
                SpaceId::Initial,
                initial_keys.clone(),
                initial_space.clone(),
                rcvd_conn_frames.clone(),
                initial_space_frame_queue,
                true,
            ),
        );

        let (handshake_pkt_tx, handshake_pkt_rx) =
            mpsc::unbounded_channel::<(HandshakePacket, ArcPath)>();
        let (handshake_ack_tx, handshake_ack_rx) = mpsc::unbounded_channel();
        let (handshake_loss_tx, handshake_loss_rx) = mpsc::unbounded_channel();
        let handshake_crypto_stream = CryptoStream::new(1000_000, 1000_000);
        let handshake_crypto_handler = handshake_crypto_stream.split();
        let handshake_keys = ArcKeys::new_pending();
        let handshake_space_frame_queue = ArcFrameQueue::new();
        let handshake_space = ArcSpace::new_initial_space(
            handshake_crypto_stream,
            handshake_ack_rx,
            handshake_loss_rx,
            handshake_space_frame_queue.clone(),
        );
        tokio::spawn(
            auto::loop_read_long_packet_and_then_dispatch_to_space_frame_queue(
                handshake_pkt_rx,
                SpaceId::Handshake,
                handshake_keys.clone(),
                handshake_space.clone(),
                rcvd_conn_frames.clone(),
                handshake_space_frame_queue,
                true,
            ),
        );
        tokio::spawn(
            handshake::exchange_initial_crypto_msg_until_getting_handshake_key(
                tls_session.clone(),
                handshake_keys.clone(),
                initial_crypto_handler,
            ),
        );

        let (zero_rtt_pkt_tx, zero_rtt_pkt_rx) =
            mpsc::unbounded_channel::<(ZeroRttPacket, ArcPath)>();
        let (one_rtt_pkt_tx, one_rtt_pkt_rx) = mpsc::unbounded_channel::<(OneRttPacket, ArcPath)>();
        let zero_rtt_keys = ArcKeys::new_pending();
        let one_rtt_keys = ArcOneRttKeys::new_pending();
        let one_rtt_crypto_stream = CryptoStream::new(1000_000, 1000_000);
        let _one_rtt_crypto_handler = one_rtt_crypto_stream.split();
        let data_space_frame_queue = ArcFrameQueue::new();
        let (data_ack_tx, data_ack_rx) = mpsc::unbounded_channel();
        let (data_loss_tx, data_loss_rx) = mpsc::unbounded_channel();
        let sending_frames = Arc::new(Mutex::new(VecDeque::new()));
        let streams =
            DataStreams::with_role_and_limit(Role::Client, 20, 10, sending_frames.clone());
        let data_space = ArcSpace::new_data_space(
            one_rtt_crypto_stream,
            streams,
            data_ack_rx,
            data_loss_rx,
            data_space_frame_queue.clone(),
        );
        tokio::spawn(
            auto::loop_read_long_packet_and_then_dispatch_to_space_frame_queue(
                zero_rtt_pkt_rx,
                SpaceId::ZeroRtt,
                zero_rtt_keys.clone(),
                data_space.clone(),
                rcvd_conn_frames.clone(),
                data_space_frame_queue.clone(),
                false,
            ),
        );
        tokio::spawn(
            auto::loop_read_short_packet_and_then_dispatch_to_space_frame_queue(
                one_rtt_pkt_rx,
                one_rtt_keys.clone(),
                data_space.clone(),
                rcvd_conn_frames.clone(),
                data_space_frame_queue,
            ),
        );
        tokio::spawn(
            handshake::exchange_handshake_crypto_msg_until_getting_1rtt_key(
                tls_session,
                one_rtt_keys,
                handshake_crypto_handler,
            ),
        );

        Self {
            initial_keys,
            initial_pkt_queue: Some(initial_pkt_tx),
            initial_space,
            handshake_keys,
            handshake_pkt_queue: Some(handshake_pkt_tx),
            handshake_space,
            zero_rtt_keys,
            zero_rtt_pkt_queue: Some(zero_rtt_pkt_tx),
            one_rtt_pkt_queue: one_rtt_pkt_tx,
            data_space,
            spin: SpinBit::default(),
        }
    }

    pub fn recv_initial_packet(&mut self, pkt: InitialPacket, path: ArcPath) {
        self.initial_pkt_queue.as_mut().map(|q| {
            let _ = q.send((pkt, path));
        });
    }

    pub fn recv_handshake_packet(&mut self, pkt: HandshakePacket, path: ArcPath) {
        self.handshake_pkt_queue.as_mut().map(|q| {
            let _ = q.send((pkt, path));
        });
    }

    pub fn recv_0rtt_packet(&mut self, pkt: ZeroRttPacket, path: ArcPath) {
        self.zero_rtt_pkt_queue.as_mut().map(|q| {
            let _ = q.send((pkt, path));
        });
    }

    pub fn recv_1rtt_packet(&mut self, pkt: OneRttPacket, path: ArcPath) {
        self.one_rtt_pkt_queue
            .send((pkt, path))
            .expect("must success");
    }

    pub fn invalid_initial_keys(&self) {
        self.initial_keys.invalid();
    }

    pub fn invalid_handshake_keys(&self) {
        self.handshake_keys.invalid();
    }

    pub fn invalid_zero_rtt_keys(&self) {
        self.zero_rtt_keys.invalid();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        assert_eq!(2 + 2, 4)
    }
}
