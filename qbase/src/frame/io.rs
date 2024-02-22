use super::{
    ack::ack_frame_with_flag, connection_close::connection_close_frame_at_layer,
    crypto::be_crypto_frame, data_blocked::be_data_blocked_frame, max_data::be_max_data_frame,
    max_stream_data::be_max_stream_data_frame, max_streams::max_streams_frame_with_dir,
    new_connection_id::be_new_connection_id_frame, new_token::be_new_token_frame,
    new_token::WriteNewTokenFrame, path_challenge::be_path_challenge_frame,
    path_response::be_path_response_frame, reset_stream::be_reset_stream_frame,
    retire_connection_id::be_retire_connection_id_frame, stop_sending::be_stop_sending_frame,
    stream::stream_frame_with_flag, stream_data_blocked::be_stream_data_blocked_frame,
    streams_blocked::streams_blocked_frame_with_dir, *,
};
use bytes::Bytes;

/// Some frames like `STREAM` and `CRYPTO` have a data body, which use `bytes::Bytes` to store.
fn complete_frame(
    frame_type: FrameType,
    raw: Bytes,
) -> impl Fn(&[u8]) -> nom::IResult<&[u8], Frame> {
    use nom::combinator::map;
    move |input: &[u8]| match frame_type {
        FrameType::Padding => Ok((input, Frame::Pure(PureFrame::Padding(PaddingFrame)))),
        FrameType::Ping => Ok((input, Frame::Pure(PureFrame::Ping(PingFrame)))),
        FrameType::ConnectionClose(layer) => map(connection_close_frame_at_layer(layer), |f| {
            Frame::Pure(PureFrame::Conn(ConnFrame::Close(f)))
        })(input),
        FrameType::NewConnectionId => map(be_new_connection_id_frame, |f| {
            Frame::Pure(PureFrame::Conn(ConnFrame::NewConnectionId(f)))
        })(input),
        FrameType::RetireConnectionId => map(be_retire_connection_id_frame, |f| {
            Frame::Pure(PureFrame::Conn(ConnFrame::RetireConnectionId(f)))
        })(input),
        FrameType::DataBlocked => map(be_data_blocked_frame, |f| {
            Frame::Pure(PureFrame::Conn(ConnFrame::DataBlocked(f)))
        })(input),
        FrameType::MaxData => map(be_max_data_frame, |f| {
            Frame::Pure(PureFrame::Conn(ConnFrame::MaxData(f)))
        })(input),
        FrameType::PathChallenge => map(be_path_challenge_frame, |f| {
            Frame::Pure(PureFrame::Path(PathFrame::Challenge(f)))
        })(input),
        FrameType::PathResponse => map(be_path_response_frame, |f| {
            Frame::Pure(PureFrame::Path(PathFrame::Response(f)))
        })(input),
        FrameType::HandshakeDone => Ok((
            input,
            Frame::Pure(PureFrame::Conn(ConnFrame::HandshakeDone(
                HandshakeDoneFrame,
            ))),
        )),
        FrameType::NewToken => map(be_new_token_frame, |f| {
            Frame::Pure(PureFrame::Conn(ConnFrame::NewToken(f)))
        })(input),
        FrameType::Ack(ecn) => {
            map(ack_frame_with_flag(ecn), |f| Frame::Pure(PureFrame::Ack(f)))(input)
        }
        FrameType::ResetStream => map(be_reset_stream_frame, |f| {
            Frame::Pure(PureFrame::Stream(f.into()))
        })(input),
        FrameType::StopSending => map(be_stop_sending_frame, |f| {
            Frame::Pure(PureFrame::Stream(f.into()))
        })(input),
        FrameType::MaxStreamData => map(be_max_stream_data_frame, |f| {
            Frame::Pure(PureFrame::Stream(f.into()))
        })(input),
        FrameType::MaxStreams(dir) => map(max_streams_frame_with_dir(dir), |f| {
            Frame::Pure(PureFrame::Stream(f.into()))
        })(input),
        FrameType::StreamsBlocked(dir) => map(streams_blocked_frame_with_dir(dir), |f| {
            Frame::Pure(PureFrame::Stream(f.into()))
        })(input),
        FrameType::StreamDataBlocked => map(be_stream_data_blocked_frame, |f| {
            Frame::Pure(PureFrame::Stream(f.into()))
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

pub fn be_frame(raw: &Bytes) -> Result<(usize, Frame), Error> {
    use crate::varint::be_varint;
    let input = raw.as_ref();
    let (remain, fty) = be_varint(input).map_err(|e| match e {
        ne @ nom::Err::Incomplete(_) => nom::Err::Error(Error::IncompleteType(ne.to_string())),
        _ => unreachable!("parsing frame type which is a varint never generates error or failure"),
    })?;
    let frame_type = FrameType::try_from(fty).map_err(nom::Err::Error)?;
    let (remain, frame) = complete_frame(frame_type, raw.clone())(remain).map_err(|e| match e {
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

use super::{
    data_blocked::WriteDataBlockedFrame, handshake_done::WriteHandshakeDoneFrame,
    max_data::WriteMaxDataFrame, max_stream_data::WriteMaxStreamDataFrame,
    max_streams::WriteMaxStreamsFrame, new_connection_id::WriteNewConnectionIdFrame,
    path_challenge::WritePathChallengeFrame, path_response::WritePathResponseFrame,
    reset_stream::WriteResetStreamFrame, retire_connection_id::WriteRetireConnectionIdFrame,
    stop_sending::WriteStopSendingFrame, stream_data_blocked::WriteStreamDataBlockedFrame,
    streams_blocked::WriteStreamsBlockedFrame,
};

pub use super::{
    ack::WriteAckFrame, connection_close::WriteConnectionCloseFrame, crypto::WriteCryptoFrame,
    padding::WritePaddingFrame, ping::WritePingFrame, stream::WriteStreamFrame,
};

pub trait WriteFrame<F> {
    fn put_frame(&mut self, frame: &F);
}

pub trait WriteDataFrame<D> {
    fn put_frame_with_data(&mut self, frame: &D, data: &[u8]);
}

impl<T: bytes::BufMut> WriteFrame<ConnFrame> for T {
    fn put_frame(&mut self, frame: &ConnFrame) {
        match frame {
            ConnFrame::Close(frame) => self.put_connection_close_frame(frame),
            ConnFrame::NewToken(frame) => self.put_new_token_frame(frame),
            ConnFrame::MaxData(frame) => self.put_max_data_frame(frame),
            ConnFrame::DataBlocked(frame) => self.put_data_blocked_frame(frame),
            ConnFrame::NewConnectionId(frame) => self.put_new_connection_id_frame(frame),
            ConnFrame::RetireConnectionId(frame) => self.put_retire_connection_id_frame(frame),
            ConnFrame::HandshakeDone(_) => self.put_handshake_done_frame(),
        }
    }
}

impl<T: bytes::BufMut> WriteFrame<StreamCtlFrame> for T {
    fn put_frame(&mut self, frame: &StreamCtlFrame) {
        match frame {
            StreamCtlFrame::ResetStream(frame) => self.put_reset_stream_frame(frame),
            StreamCtlFrame::StopSending(frame) => self.put_stop_sending_frame(frame),
            StreamCtlFrame::MaxStreamData(frame) => self.put_max_stream_data_frame(frame),
            StreamCtlFrame::MaxStreams(frame) => self.put_max_streams_frame(frame),
            StreamCtlFrame::StreamDataBlocked(frame) => self.put_stream_data_blocked_frame(frame),
            StreamCtlFrame::StreamsBlocked(frame) => self.put_streams_blocked_frame(frame),
        }
    }
}

impl<T: bytes::BufMut> WriteFrame<PathFrame> for T {
    fn put_frame(&mut self, frame: &PathFrame) {
        match frame {
            PathFrame::Challenge(frame) => self.put_path_challenge_frame(frame),
            PathFrame::Response(frame) => self.put_path_response_frame(frame),
        }
    }
}

impl<T: bytes::BufMut> WriteFrame<PureFrame> for T {
    fn put_frame(&mut self, frame: &PureFrame) {
        match frame {
            PureFrame::Padding(_) => self.put_padding_frame(),
            PureFrame::Ping(_) => self.put_ping_frame(),
            PureFrame::Ack(frame) => self.put_ack_frame(frame),
            PureFrame::Conn(frame) => self.put_frame(frame),
            PureFrame::Stream(frame) => self.put_frame(frame),
            PureFrame::Path(frame) => self.put_frame(frame),
        }
    }
}

impl<T: bytes::BufMut> WriteFrame<ReliableFrame> for T {
    fn put_frame(&mut self, frame: &ReliableFrame) {
        match frame {
            ReliableFrame::Conn(frame) => self.put_frame(frame),
            ReliableFrame::Stream(frame) => self.put_frame(frame),
        }
    }
}

impl<T: bytes::BufMut> WriteDataFrame<CryptoFrame> for T {
    fn put_frame_with_data(&mut self, frame: &CryptoFrame, data: &[u8]) {
        self.put_crypto_frame(frame, data);
    }
}

impl<T: bytes::BufMut> WriteDataFrame<StreamFrame> for T {
    fn put_frame_with_data(&mut self, frame: &StreamFrame, data: &[u8]) {
        self.put_stream_frame(frame, data);
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
