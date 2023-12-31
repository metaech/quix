// This folder defines all the frames, including their parsing and packaging processes.
use enum_dispatch::enum_dispatch;

#[enum_dispatch]
pub trait BeFrame {
    fn frame_type(&self) -> FrameType;
    fn max_encoding_size(&self) -> usize {
        1
    }

    fn encoding_size(&self) -> usize {
        1
    }
}

mod ack;
mod connection_close;
mod crypto;
mod data_blocked;
mod handshake_done;
mod max_data;
mod max_stream_data;
mod max_streams;
mod new_connection_id;
mod new_token;
mod padding;
mod path_challenge;
mod path_response;
mod ping;
mod reset_stream;
mod retire_connection_id;
mod stop_sending;
mod stream;
mod stream_data_blocked;
mod streams_blocked;

pub mod error;
pub use error::Error;

// re-export for convenience
pub use ack::{AckFrame, AckRecord};
pub use connection_close::ConnectionCloseFrame;
pub use crypto::CryptoFrame;
pub use data_blocked::DataBlockedFrame;
pub use handshake_done::HandshakeDoneFrame;
pub use max_data::MaxDataFrame;
pub use max_stream_data::MaxStreamDataFrame;
pub use max_streams::MaxStreamsFrame;
pub use new_connection_id::NewConnectionIdFrame;
pub use new_token::NewTokenFrame;
pub use padding::PaddingFrame;
pub use path_challenge::PathChallengeFrame;
pub use path_response::PathResponseFrame;
pub use ping::PingFrame;
pub use reset_stream::ResetStreamFrame;
pub use retire_connection_id::RetireConnectionIdFrame;
pub use stop_sending::StopSendingFrame;
pub use stream::{ShouldCarryLength, StreamFrame};
pub use stream_data_blocked::StreamDataBlockedFrame;
pub use streams_blocked::StreamsBlockedFrame;

use super::varint::VarInt;
use bytes::{Buf, Bytes};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum FrameType {
    Padding,
    Ping,
    Ack(u8),
    ResetStream,
    StopSending,
    Crypto,
    NewToken,
    Stream(u8),
    MaxData,
    MaxStreamData,
    MaxStreams(u8),
    DataBlocked,
    StreamDataBlocked,
    StreamsBlocked(u8),
    NewConnectionId,
    RetireConnectionId,
    PathChallenge,
    PathResponse,
    ConnectionClose(u8),
    HandshakeDone,
}

impl TryFrom<VarInt> for FrameType {
    type Error = Error;

    fn try_from(frame_type: VarInt) -> Result<Self, Self::Error> {
        Ok(match frame_type.into_inner() {
            0x00 => FrameType::Padding,
            0x01 => FrameType::Ping,
            ty @ (0x02 | 0x03) => FrameType::Ack(ty as u8 & 0b1),
            0x04 => FrameType::ResetStream,
            0x05 => FrameType::StopSending,
            0x06 => FrameType::Crypto,
            0x07 => FrameType::NewToken,
            ty @ 0x08..=0x0f => FrameType::Stream(ty as u8 & 0b111),
            0x10 => FrameType::MaxData,
            0x11 => FrameType::MaxStreamData,
            ty @ (0x12 | 0x13) => FrameType::MaxStreams(ty as u8 & 0b1),
            0x14 => FrameType::DataBlocked,
            0x15 => FrameType::StreamDataBlocked,
            ty @ (0x16 | 0x17) => FrameType::StreamsBlocked(ty as u8 & 0b1),
            0x18 => FrameType::NewConnectionId,
            0x19 => FrameType::RetireConnectionId,
            0x1a => FrameType::PathChallenge,
            0x1b => FrameType::PathResponse,
            ty @ (0x1c | 0x1d) => FrameType::ConnectionClose(ty as u8 & 0x1),
            0x1e => FrameType::HandshakeDone,
            _ => return Err(Self::Error::InvalidType(frame_type)),
        })
    }
}

impl From<FrameType> for VarInt {
    fn from(frame_type: FrameType) -> Self {
        match frame_type {
            FrameType::Padding => VarInt(0x00),
            FrameType::Ping => VarInt(0x01),
            FrameType::Ack(ecn) => VarInt(0x02 | ecn as u64),
            FrameType::ResetStream => VarInt(0x04),
            FrameType::StopSending => VarInt(0x05),
            FrameType::Crypto => VarInt(0x06),
            FrameType::NewToken => VarInt(0x07),
            FrameType::Stream(flag) => VarInt(0x08 | flag as u64),
            FrameType::MaxData => VarInt(0x10),
            FrameType::MaxStreamData => VarInt(0x11),
            FrameType::MaxStreams(dir) => VarInt(0x12 | dir as u64),
            FrameType::DataBlocked => VarInt(0x14),
            FrameType::StreamDataBlocked => VarInt(0x15),
            FrameType::StreamsBlocked(dir) => VarInt(0x16 | dir as u64),
            FrameType::NewConnectionId => VarInt(0x18),
            FrameType::RetireConnectionId => VarInt(0x19),
            FrameType::PathChallenge => VarInt(0x1a),
            FrameType::PathResponse => VarInt(0x1b),
            FrameType::ConnectionClose(layer) => VarInt(0x1c | layer as u64),
            FrameType::HandshakeDone => VarInt(0x1e),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[enum_dispatch(BeFrame)]
pub enum StreamInfoFrame {
    ResetStream(ResetStreamFrame),
    StopSending(StopSendingFrame),
    MaxStreamData(MaxStreamDataFrame),
    MaxStreams(MaxStreamsFrame),
    StreamDataBlocked(StreamDataBlockedFrame),
    StreamsBlocked(StreamsBlockedFrame),
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[enum_dispatch(BeFrame)]
pub enum InfoFrame {
    Ping(PingFrame),
    NewToken(NewTokenFrame),
    MaxData(MaxDataFrame),
    DataBlocked(DataBlockedFrame),
    NewConnectionId(NewConnectionIdFrame),
    RetireConnectionId(RetireConnectionIdFrame),
    PathChallenge(PathChallengeFrame),
    PathResponse(PathResponseFrame),
    HandshakeDone(HandshakeDoneFrame),
    Stream(StreamInfoFrame),
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[enum_dispatch(BeFrame)]
pub enum ConnectionFrame {
    Close(ConnectionCloseFrame),
    NewToken(NewTokenFrame),
    MaxData(MaxDataFrame),
    DataBlocked(DataBlockedFrame),
    NewConnectionId(NewConnectionIdFrame),
    RetireConnectionId(RetireConnectionIdFrame),
    PathChallenge(PathChallengeFrame),
    PathResponse(PathResponseFrame),
    HandshakeDone(HandshakeDoneFrame),
}

// The initial packet and handshake packet only contain Ping frame.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct NoFrame;

impl TryFrom<InfoFrame> for NoFrame {
    type Error = Error;

    fn try_from(value: InfoFrame) -> Result<Self, Self::Error> {
        Err(Self::Error::WrongFrame(
            value.frame_type(),
            "Initial or Handshake",
        ))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[enum_dispatch(BeFrame)]
pub enum ZeroRttFrame {
    Ping(PingFrame), // 放在这里，应是没用?只可能想要写Ping帧时用得着
    MaxData(MaxDataFrame),
    DataBlocked(DataBlockedFrame),
    NewConnectionId(NewConnectionIdFrame),
    RetireConnectionId(RetireConnectionIdFrame),
    PathChallenge(PathChallengeFrame),
    Stream(StreamInfoFrame),
}

impl TryFrom<InfoFrame> for ZeroRttFrame {
    type Error = Error;

    fn try_from(value: InfoFrame) -> Result<Self, Self::Error> {
        match value {
            InfoFrame::Ping(_) => Ok(ZeroRttFrame::Ping(PingFrame)),
            InfoFrame::MaxData(frame) => Ok(ZeroRttFrame::MaxData(frame)),
            InfoFrame::DataBlocked(frame) => Ok(ZeroRttFrame::DataBlocked(frame)),
            InfoFrame::NewConnectionId(frame) => Ok(ZeroRttFrame::NewConnectionId(frame)),
            InfoFrame::RetireConnectionId(frame) => Ok(ZeroRttFrame::RetireConnectionId(frame)),
            InfoFrame::PathChallenge(frame) => Ok(ZeroRttFrame::PathChallenge(frame)),
            InfoFrame::Stream(frame) => Ok(ZeroRttFrame::Stream(frame)),
            other => Err(Self::Error::WrongFrame(other.frame_type(), "Zero rtt data")),
        }
    }
}

impl From<ZeroRttFrame> for InfoFrame {
    fn from(value: ZeroRttFrame) -> Self {
        match value {
            ZeroRttFrame::Ping(_) => InfoFrame::Ping(PingFrame),
            ZeroRttFrame::MaxData(frame) => InfoFrame::MaxData(frame),
            ZeroRttFrame::DataBlocked(frame) => InfoFrame::DataBlocked(frame),
            ZeroRttFrame::NewConnectionId(frame) => InfoFrame::NewConnectionId(frame),
            ZeroRttFrame::RetireConnectionId(frame) => InfoFrame::RetireConnectionId(frame),
            ZeroRttFrame::PathChallenge(frame) => InfoFrame::PathChallenge(frame),
            ZeroRttFrame::Stream(frame) => InfoFrame::Stream(frame),
        }
    }
}

pub type OneRttFrame = InfoFrame;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum DataFrame {
    Crypto(CryptoFrame),
    Stream(StreamFrame),
}

impl TryFrom<DataFrame> for CryptoFrame {
    type Error = Error;

    fn try_from(value: DataFrame) -> Result<Self, Self::Error> {
        match value {
            DataFrame::Crypto(frame) => Ok(frame),
            DataFrame::Stream(f) => Err(Self::Error::WrongData(
                f.frame_type(),
                "Initail or Handshake",
            )),
        }
    }
}

impl TryFrom<DataFrame> for StreamFrame {
    type Error = Error;

    fn try_from(value: DataFrame) -> Result<Self, Self::Error> {
        match value {
            DataFrame::Stream(frame) => Ok(frame),
            DataFrame::Crypto(_) => Err(Self::Error::WrongData(FrameType::Crypto, "Zero rtt data")),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Frame {
    Padding,
    Ack(AckFrame),
    Close(ConnectionCloseFrame),
    Info(InfoFrame),
    Data(DataFrame, Bytes),
}

impl Frame {
    pub fn is_conn_layer_interest(&self) -> bool {
        match self {
            Self::Close(_) => true,
            Self::Info(info) => !matches!(info, InfoFrame::Ping(_) | InfoFrame::Stream(_)),
            _ => false,
        }
    }
}

pub struct FrameReader {
    raw: Bytes,
}

impl FrameReader {
    pub fn new(raw: Bytes) -> Self {
        Self { raw }
    }
}

impl Iterator for FrameReader {
    type Item = Result<Frame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.raw.is_empty() {
            return None;
        }

        match ext::be_frame(&self.raw) {
            Ok((consumed, frame)) => {
                self.raw.advance(consumed);
                Some(Ok(frame))
            }
            Err(e) => {
                self.raw.clear(); // no longer parsing
                Some(Err(e))
            }
        }
    }
}

pub mod ext {
    use super::*;
    use super::{
        ack::ext::ack_frame_with_flag, connection_close::ext::connection_close_frame_at_layer,
        crypto::ext::be_crypto_frame, data_blocked::ext::be_data_blocked_frame,
        max_data::ext::be_max_data_frame, max_stream_data::ext::be_max_stream_data_frame,
        max_streams::ext::max_streams_frame_with_dir,
        new_connection_id::ext::be_new_connection_id_frame, new_token::ext::be_new_token_frame,
        path_challenge::ext::be_path_challenge_frame, path_response::ext::be_path_response_frame,
        reset_stream::ext::be_reset_stream_frame,
        retire_connection_id::ext::be_retire_connection_id_frame,
        stop_sending::ext::be_stop_sending_frame, stream::ext::stream_frame_with_flag,
        stream_data_blocked::ext::be_stream_data_blocked_frame,
        streams_blocked::ext::streams_blocked_frame_with_dir,
    };

    use bytes::Bytes;
    use nom::{
        combinator::{eof, map},
        multi::many_till,
    };

    /// Some frames like `STREAM` and `CRYPTO` have a data body, which use `bytes::Bytes` to store.
    fn complete_frame(
        frame_type: FrameType,
        raw: Bytes,
    ) -> impl Fn(&[u8]) -> nom::IResult<&[u8], Frame> {
        move |input: &[u8]| match frame_type {
            FrameType::Padding => Ok((input, Frame::Padding)),
            FrameType::Ping => Ok((input, Frame::Info(InfoFrame::Ping(PingFrame)))),
            FrameType::ConnectionClose(layer) => {
                map(connection_close_frame_at_layer(layer), Frame::Close)(input)
            }
            FrameType::NewConnectionId => map(be_new_connection_id_frame, |f| {
                Frame::Info(InfoFrame::NewConnectionId(f))
            })(input),
            FrameType::RetireConnectionId => map(be_retire_connection_id_frame, |f| {
                Frame::Info(InfoFrame::RetireConnectionId(f))
            })(input),
            FrameType::PathChallenge => map(be_path_challenge_frame, |f| {
                Frame::Info(InfoFrame::PathChallenge(f))
            })(input),
            FrameType::PathResponse => map(be_path_response_frame, |f| {
                Frame::Info(InfoFrame::PathResponse(f))
            })(input),
            FrameType::HandshakeDone => Ok((
                input,
                Frame::Info(InfoFrame::HandshakeDone(HandshakeDoneFrame)),
            )),
            FrameType::NewToken => {
                map(be_new_token_frame, |f| Frame::Info(InfoFrame::NewToken(f)))(input)
            }
            FrameType::Ack(ecn) => map(ack_frame_with_flag(ecn), Frame::Ack)(input),
            FrameType::ResetStream => map(be_reset_stream_frame, |f| {
                Frame::Info(InfoFrame::Stream(f.into()))
            })(input),
            FrameType::DataBlocked => map(be_data_blocked_frame, |f| {
                Frame::Info(InfoFrame::DataBlocked(f))
            })(input),
            FrameType::MaxData => {
                map(be_max_data_frame, |f| Frame::Info(InfoFrame::MaxData(f)))(input)
            }
            FrameType::StopSending => map(be_stop_sending_frame, |f| {
                Frame::Info(InfoFrame::Stream(f.into()))
            })(input),
            FrameType::MaxStreamData => map(be_max_stream_data_frame, |f| {
                Frame::Info(InfoFrame::Stream(f.into()))
            })(input),
            FrameType::MaxStreams(dir) => map(max_streams_frame_with_dir(dir), |f| {
                Frame::Info(InfoFrame::Stream(f.into()))
            })(input),
            FrameType::StreamsBlocked(dir) => map(streams_blocked_frame_with_dir(dir), |f| {
                Frame::Info(InfoFrame::Stream(f.into()))
            })(input),
            FrameType::StreamDataBlocked => map(be_stream_data_blocked_frame, |f| {
                Frame::Info(InfoFrame::Stream(f.into()))
            })(input),
            FrameType::Crypto => {
                let (input, frame) = be_crypto_frame(input)?;
                let start = raw.len() - input.len();
                let len = frame.length.into_inner() as usize;
                if input.len() < len {
                    Err(nom::Err::Incomplete(nom::Needed::new(len - input.len())))
                } else {
                    let data = raw.slice(start..start + len);
                    Ok((&input[len..], Frame::Data(DataFrame::Crypto(frame), data)))
                }
            }
            FrameType::Stream(flag) => {
                let (input, frame) = stream_frame_with_flag(flag)(input)?;
                let start = raw.len() - input.len();
                let len = frame.length;
                if input.len() < len {
                    Err(nom::Err::Incomplete(nom::Needed::new(len - input.len())))
                } else {
                    let data = raw.slice(start..start + len);
                    Ok((&input[len..], Frame::Data(DataFrame::Stream(frame), data)))
                }
            }
        }
    }

    pub(super) fn be_frame(raw: &Bytes) -> Result<(usize, Frame), Error> {
        use crate::varint::ext::be_varint;
        let input = raw.as_ref();
        let (remain, fty) = be_varint(input).map_err(|e| match e {
            ne @ nom::Err::Incomplete(_) => nom::Err::Error(Error::IncompleteType(ne.to_string())),
            _ => unreachable!(
                "parsing frame type which is a varint never generates error or failure"
            ),
        })?;
        let frame_type = FrameType::try_from(fty).map_err(nom::Err::Error)?;
        let (remain, frame) =
            complete_frame(frame_type, raw.clone())(remain).map_err(|e| match e {
                ne @ nom::Err::Incomplete(_) => {
                    nom::Err::Error(Error::IncompleteFrame(frame_type, ne.to_string()))
                }
                nom::Err::Error(ne) => {
                    // may be TooLarge in MaxStreamsFrame/CryptoFrame/StreamFrame,
                    // or may be Verify in NewConnectionIdFrame,
                    // or may be Alt in ConnectionCloseFrame
                    nom::Err::Error(Error::ParseError(
                        frame_type,
                        ne.code.description().to_owned(),
                    ))
                }
                _ => unreachable!("parsing frame never fails"),
            })?;
        Ok((input.len() - remain.len(), frame))
    }

    // nom parser for FRAME
    pub fn be_frame_deprecated(
        raw: Bytes,
    ) -> impl FnMut(&[u8]) -> nom::IResult<&[u8], Frame, Error> {
        move |input: &[u8]| {
            use crate::varint::ext::be_varint;
            let (input, fty) = be_varint(input).map_err(|e| match e {
                ne @ nom::Err::Incomplete(_) => {
                    nom::Err::Error(Error::IncompleteType(ne.to_string()))
                }
                _ => unreachable!(
                    "parsing frame type which is a varint never generates error or failure"
                ),
            })?;
            let frame_type = FrameType::try_from(fty).map_err(nom::Err::Error)?;
            complete_frame(frame_type, raw.clone())(input).map_err(|e| match e {
                ne @ nom::Err::Incomplete(_) => {
                    nom::Err::Error(Error::IncompleteFrame(frame_type, ne.to_string()))
                }
                nom::Err::Error(ne) => {
                    // may be TooLarge in MaxStreamsFrame/CryptoFrame/StreamFrame,
                    // or may be Verify in NewConnectionIdFrame,
                    // or may be Alt in ConnectionCloseFrame
                    nom::Err::Error(Error::ParseError(
                        frame_type,
                        ne.code.description().to_owned(),
                    ))
                }
                _ => unreachable!("parsing frame never fails"),
            })
        }
    }

    pub fn parse_frames_from_bytes(bytes: Bytes) -> Result<Vec<Frame>, Error> {
        let raw = bytes.clone();
        let input = bytes.as_ref();
        // many1 cannot check if it has reached EOF or if the last frame is incomplete;
        // many_till eof cannot check if it contains at least one.
        let (_, (frames, _)) = many_till(be_frame_deprecated(raw), eof)(input)?;
        if frames.is_empty() {
            return Err(Error::NoFrames);
        }
        Ok(frames)
    }

    use super::{
        data_blocked::ext::WriteDataBlockedFrame, handshake_done::ext::WriteHandshakeDoneFrame,
        max_data::ext::WriteMaxDataFrame, max_stream_data::ext::WriteMaxStreamDataFrame,
        max_streams::ext::WriteMaxStreamsFrame, new_connection_id::ext::WriteNewConnectionIdFrame,
        new_token::ext::WriteNewTokenFrame, path_challenge::ext::WritePathChallengeFrame,
        path_response::ext::WritePathResponseFrame, reset_stream::ext::WriteResetStreamFrame,
        retire_connection_id::ext::WriteRetireConnectionIdFrame,
        stop_sending::ext::WriteStopSendingFrame,
        stream_data_blocked::ext::WriteStreamDataBlockedFrame,
        streams_blocked::ext::WriteStreamsBlockedFrame,
    };

    pub use super::{
        ack::ext::WriteAckFrame, connection_close::ext::WriteConnectionCloseFrame,
        crypto::ext::WriteCryptoFrame, padding::ext::WritePaddingFrame, ping::ext::WritePingFrame,
        stream::ext::WriteStreamFrame,
    };

    pub trait WriteFrame<F> {
        fn put_frame(&mut self, frame: &F);
    }

    pub trait WriteDataFrame<D> {
        fn put_frame_with_data(&mut self, frame: &D, data: &[u8]);
    }

    impl<T: bytes::BufMut> WriteFrame<NoFrame> for T {
        fn put_frame(&mut self, _frame: &NoFrame) {
            // do nothing
        }
    }

    impl<T: bytes::BufMut> WriteFrame<StreamInfoFrame> for T {
        fn put_frame(&mut self, frame: &StreamInfoFrame) {
            match frame {
                StreamInfoFrame::ResetStream(frame) => self.put_reset_stream_frame(frame),
                StreamInfoFrame::StopSending(frame) => self.put_stop_sending_frame(frame),
                StreamInfoFrame::MaxStreamData(frame) => self.put_max_stream_data_frame(frame),
                StreamInfoFrame::MaxStreams(frame) => self.put_max_streams_frame(frame),
                StreamInfoFrame::StreamDataBlocked(frame) => {
                    self.put_stream_data_blocked_frame(frame)
                }
                StreamInfoFrame::StreamsBlocked(frame) => self.put_streams_blocked_frame(frame),
            }
        }
    }

    impl<T: bytes::BufMut> WriteDataFrame<CryptoFrame> for T {
        fn put_frame_with_data(&mut self, frame: &CryptoFrame, data: &[u8]) {
            self.put_crypto_frame(frame, data);
        }
    }

    impl<T: bytes::BufMut> WriteFrame<ZeroRttFrame> for T {
        fn put_frame(&mut self, frame: &ZeroRttFrame) {
            match frame {
                ZeroRttFrame::Ping(_) => self.put_ping_frame(),
                ZeroRttFrame::MaxData(frame) => self.put_max_data_frame(frame),
                ZeroRttFrame::DataBlocked(frame) => self.put_data_blocked_frame(frame),
                ZeroRttFrame::NewConnectionId(frame) => self.put_new_connection_id_frame(frame),
                ZeroRttFrame::RetireConnectionId(frame) => {
                    self.put_retire_connection_id_frame(frame)
                }
                ZeroRttFrame::PathChallenge(frame) => self.put_path_challenge_frame(frame),
                ZeroRttFrame::Stream(frame) => {
                    (self as &mut dyn WriteFrame<StreamInfoFrame>).put_frame(frame)
                }
            }
        }
    }

    impl<T: bytes::BufMut> WriteDataFrame<StreamFrame> for T {
        fn put_frame_with_data(&mut self, frame: &StreamFrame, data: &[u8]) {
            self.put_stream_frame(frame, data);
        }
    }

    impl<T: bytes::BufMut> WriteFrame<InfoFrame> for T {
        fn put_frame(&mut self, frame: &InfoFrame) {
            match frame {
                InfoFrame::Ping(_) => self.put_ping_frame(),
                InfoFrame::NewToken(frame) => self.put_new_token_frame(frame),
                InfoFrame::MaxData(frame) => self.put_max_data_frame(frame),
                InfoFrame::DataBlocked(frame) => self.put_data_blocked_frame(frame),
                InfoFrame::NewConnectionId(frame) => self.put_new_connection_id_frame(frame),
                InfoFrame::RetireConnectionId(frame) => self.put_retire_connection_id_frame(frame),
                InfoFrame::PathChallenge(frame) => self.put_path_challenge_frame(frame),
                InfoFrame::PathResponse(frame) => self.put_path_response_frame(frame),
                InfoFrame::HandshakeDone(_) => self.put_handshake_done_frame(),
                InfoFrame::Stream(frame) => {
                    (self as &mut dyn WriteFrame<StreamInfoFrame>).put_frame(frame)
                }
            }
        }
    }

    impl<T: bytes::BufMut> WriteDataFrame<DataFrame> for T {
        fn put_frame_with_data(&mut self, frame: &DataFrame, data: &[u8]) {
            match frame {
                DataFrame::Crypto(frame) => self.put_crypto_frame(frame, data),
                DataFrame::Stream(frame) => self.put_stream_frame(frame, data),
            }
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
